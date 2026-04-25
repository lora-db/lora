use std::fmt;

use serde::{Deserialize, Serialize};

/// Monotonic log sequence number.
///
/// LSN 0 is reserved for "empty / never written" so a snapshot's
/// `wal_lsn = 0` cannot be mistaken for "I checkpointed at the very first
/// record". Allocators advance from 1.
///
/// Internally an LSN is opaque: callers only rely on the total order. The
/// representation is left as a single `u64` for now; a future change to a
/// `(segment_id, offset)` packing is non-breaking because every consumer
/// goes through these accessors.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[repr(transparent)]
pub struct Lsn(u64);

impl Lsn {
    pub const ZERO: Lsn = Lsn(0);

    pub const fn new(value: u64) -> Self {
        Self(value)
    }

    pub const fn raw(self) -> u64 {
        self.0
    }

    pub const fn is_zero(self) -> bool {
        self.0 == 0
    }

    /// Returns the next LSN. Panics on `u64::MAX` — saturating would
    /// silently violate monotonicity, and overflow at this scale means a
    /// trillion records per second for ~580 million years.
    pub fn next(self) -> Self {
        Self(
            self.0
                .checked_add(1)
                .expect("Lsn overflowed; the WAL has been continuously running for ~580 My"),
        )
    }
}

impl fmt::Display for Lsn {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl From<u64> for Lsn {
    fn from(value: u64) -> Self {
        Self(value)
    }
}

impl From<Lsn> for u64 {
    fn from(value: Lsn) -> Self {
        value.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn next_advances_monotonically() {
        let a = Lsn::new(7);
        let b = a.next();
        assert!(b > a);
        assert_eq!(b.raw(), 8);
    }

    #[test]
    fn zero_is_sentinel() {
        assert!(Lsn::ZERO.is_zero());
        assert!(!Lsn::new(1).is_zero());
    }

    #[test]
    #[should_panic(expected = "Lsn overflowed")]
    fn overflow_panics() {
        let _ = Lsn::new(u64::MAX).next();
    }
}
