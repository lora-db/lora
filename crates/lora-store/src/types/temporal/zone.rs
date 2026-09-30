//! [`ZoneId`]: a named IANA time zone (`Europe/Amsterdam`), resolved
//! against the time zone database bundled into the binary by `jiff`, so
//! it works the same on every platform and on wasm32 without a
//! filesystem.
//!
//! A `ZoneId` is a two-byte handle into a process-wide table of zone
//! names, so a zoned [`super::LoraDateTime`] stays small. The handle is
//! never persisted: codecs and serde write the zone's name.

use std::fmt;
use std::num::NonZeroU16;
use std::sync::OnceLock;

use jiff::tz::{AmbiguousOffset, TimeZone};

/// A named time zone.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ZoneId(NonZeroU16);

/// The offsets a zone gives a local date-time.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LocalOffset {
    /// One offset: the usual case.
    Unique(i32),
    /// The local time does not exist (clocks jumped forward from
    /// `before` to `after`).
    Gap { before: i32, after: i32 },
    /// The local time happens twice (clocks went back): first at
    /// `earlier`, then at `later`.
    Overlap { earlier: i32, later: i32 },
}

struct Registry {
    /// Canonical names, sorted; `ZoneId(n)` is `names[n - 1]`.
    names: Vec<&'static str>,
    zones: Vec<OnceLock<TimeZone>>,
}

fn registry() -> &'static Registry {
    static REGISTRY: OnceLock<Registry> = OnceLock::new();
    REGISTRY.get_or_init(|| {
        let mut names: Vec<&'static str> = jiff::tz::db()
            .available()
            .map(|name| &*Box::leak(name.as_str().to_owned().into_boxed_str()))
            .collect();
        names.sort_unstable();
        names.dedup();
        names.truncate(u16::MAX as usize - 1);
        let zones = names.iter().map(|_| OnceLock::new()).collect();
        Registry { names, zones }
    })
}

impl ZoneId {
    /// The zone named `name` (case-insensitive), or `None` when the
    /// database has no such zone.
    pub fn lookup(name: &str) -> Option<Self> {
        let tz = jiff::tz::db().get(name).ok()?;
        let registry = registry();
        let canonical = tz.iana_name().unwrap_or(name);
        let index = registry
            .names
            .binary_search(&canonical)
            .or_else(|_| {
                registry
                    .names
                    .binary_search_by(|n| cmp_ignore_case(n, canonical))
            })
            .ok()?;
        let id = Self(NonZeroU16::new(index as u16 + 1)?);
        let _ = registry.zones[index].set(tz);
        Some(id)
    }

    /// The zone's IANA name.
    pub fn name(self) -> &'static str {
        registry().names[self.index()]
    }

    fn index(self) -> usize {
        self.0.get() as usize - 1
    }

    fn tz(self) -> &'static TimeZone {
        let registry = registry();
        registry.zones[self.index()].get_or_init(|| {
            jiff::tz::db()
                .get(registry.names[self.index()])
                .unwrap_or(TimeZone::UTC)
        })
    }

    /// The zone's UTC offset, in seconds, at `epoch_seconds`. Instants
    /// beyond the years the database covers take the offset at its edge.
    pub fn offset_at(self, epoch_seconds: i64) -> i32 {
        let clamped = epoch_seconds.clamp(
            jiff::Timestamp::MIN.as_second(),
            jiff::Timestamp::MAX.as_second(),
        );
        let ts = jiff::Timestamp::from_second(clamped).unwrap_or(jiff::Timestamp::UNIX_EPOCH);
        self.tz().to_offset(ts).seconds()
    }

    /// The offsets the zone gives the local date-time.
    pub fn local_offset(
        self,
        year: i32,
        month: u32,
        day: u32,
        hour: u32,
        minute: u32,
        second: u32,
    ) -> LocalOffset {
        let civil = i16::try_from(year).ok().and_then(|y| {
            jiff::civil::DateTime::new(
                y,
                month as i8,
                day as i8,
                hour as i8,
                minute as i8,
                second as i8,
                0,
            )
            .ok()
        });
        let Some(civil) = civil else {
            // Out of the database's range: the offset at its edge.
            let edge = if year < 0 { i64::MIN } else { i64::MAX };
            return LocalOffset::Unique(self.offset_at(edge));
        };
        match self.tz().to_ambiguous_timestamp(civil).offset() {
            AmbiguousOffset::Unambiguous { offset } => LocalOffset::Unique(offset.seconds()),
            AmbiguousOffset::Gap { before, after } => LocalOffset::Gap {
                before: before.seconds(),
                after: after.seconds(),
            },
            AmbiguousOffset::Fold { before, after } => LocalOffset::Overlap {
                earlier: before.seconds(),
                later: after.seconds(),
            },
        }
    }
}

fn cmp_ignore_case(a: &str, b: &str) -> std::cmp::Ordering {
    a.bytes()
        .map(|c| c.to_ascii_lowercase())
        .cmp(b.bytes().map(|c| c.to_ascii_lowercase()))
}

impl fmt::Debug for ZoneId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "ZoneId({})", self.name())
    }
}

impl fmt::Display for ZoneId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

impl serde::Serialize for ZoneId {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.name())
    }
}

impl<'de> serde::Deserialize<'de> for ZoneId {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let name = String::deserialize(deserializer)?;
        Self::lookup(&name)
            .ok_or_else(|| serde::de::Error::custom(format!("unknown time zone `{name}`")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn looks_up_zones_by_name() {
        let ams = ZoneId::lookup("Europe/Amsterdam").unwrap();
        assert_eq!(ams.name(), "Europe/Amsterdam");
        assert_eq!(ZoneId::lookup("europe/amsterdam"), Some(ams));
        assert!(ZoneId::lookup("Mars/Olympus").is_none());
        // 2026-01-15 and 2026-07-15, noon UTC.
        assert_eq!(ams.offset_at(1_768_478_400), 3600);
        assert_eq!(ams.offset_at(1_784_116_800), 7200);
        let kolkata = ZoneId::lookup("Asia/Kolkata").unwrap();
        assert_eq!(kolkata.offset_at(1_784_116_800), 19_800);
    }

    #[test]
    fn local_times_in_gaps_and_overlaps() {
        let ams = ZoneId::lookup("Europe/Amsterdam").unwrap();
        assert_eq!(
            ams.local_offset(2026, 3, 29, 2, 30, 0),
            LocalOffset::Gap {
                before: 3600,
                after: 7200
            }
        );
        assert_eq!(
            ams.local_offset(2026, 10, 25, 2, 30, 0),
            LocalOffset::Overlap {
                earlier: 7200,
                later: 3600
            }
        );
        assert_eq!(
            ams.local_offset(2026, 7, 1, 12, 0, 0),
            LocalOffset::Unique(7200)
        );
    }
}
