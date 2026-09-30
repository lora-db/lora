use lora_analyzer::symbols::VarId;
use lora_store::{
    LoraBinary, LoraDate, LoraDateTime, LoraDuration, LoraLocalDateTime, LoraLocalTime, LoraPoint,
    LoraTime, LoraVector, NodeId, PropertyValue, RelationshipId, VectorValues,
};

/// A materialised path: alternating node/relationship IDs.
/// nodes.len() == rels.len() + 1
#[derive(Debug, Clone, PartialEq)]
pub struct LoraPath {
    pub nodes: Vec<NodeId>,
    pub rels: Vec<RelationshipId>,
}
use serde::ser::{SerializeMap, SerializeSeq};
use serde::{Serialize, Serializer};
use std::collections::BTreeMap;
use std::sync::Arc;

#[derive(Debug, Clone, PartialEq)]
pub enum LoraValue {
    Null,
    Bool(bool),
    Int(i64),
    Float(f64),
    String(String),
    Binary(LoraBinary),
    List(Vec<LoraValue>),
    Map(BTreeMap<String, LoraValue>),
    Node(NodeId),
    Relationship(RelationshipId),
    Path(LoraPath),
    Date(LoraDate),
    Time(LoraTime),
    LocalTime(LoraLocalTime),
    DateTime(LoraDateTime),
    LocalDateTime(LoraLocalDateTime),
    Duration(LoraDuration),
    Point(LoraPoint),
    Vector(LoraVector),
}

impl LoraValue {
    pub fn is_truthy(&self) -> bool {
        match self {
            LoraValue::Null => false,
            LoraValue::Bool(v) => *v,
            _ => true,
        }
    }

    pub fn as_i64(&self) -> Option<i64> {
        match self {
            LoraValue::Int(v) => Some(*v),
            _ => None,
        }
    }

    pub fn as_f64(&self) -> Option<f64> {
        match self {
            LoraValue::Int(v) => Some(*v as f64),
            LoraValue::Float(v) => Some(*v),
            _ => None,
        }
    }

    /// Cypher comparison of two temporal values of the same kind, by the
    /// instant (or time of day) they denote. `None` when either side is not
    /// a temporal of that kind; durations are not ordered.
    pub(crate) fn temporal_cmp(&self, other: &LoraValue) -> Option<std::cmp::Ordering> {
        let (a, b) = match (self, other) {
            (LoraValue::Date(a), LoraValue::Date(b)) => (a.order_nanos(), b.order_nanos()),
            (LoraValue::DateTime(a), LoraValue::DateTime(b)) => (a.order_nanos(), b.order_nanos()),
            (LoraValue::LocalDateTime(a), LoraValue::LocalDateTime(b)) => {
                (a.order_nanos(), b.order_nanos())
            }
            (LoraValue::Time(a), LoraValue::Time(b)) => (a.order_nanos(), b.order_nanos()),
            (LoraValue::LocalTime(a), LoraValue::LocalTime(b)) => {
                (a.order_nanos(), b.order_nanos())
            }
            _ => return None,
        };
        Some(a.cmp(&b))
    }
}

impl Serialize for LoraValue {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        match self {
            LoraValue::Null => serializer.serialize_unit(),
            LoraValue::Bool(v) => serializer.serialize_bool(*v),
            LoraValue::Int(v) => serializer.serialize_i64(*v),
            LoraValue::Float(v) => serializer.serialize_f64(*v),
            LoraValue::String(v) => serializer.serialize_str(v),
            LoraValue::Binary(v) => serialize_binary(serializer, v),

            LoraValue::List(values) => {
                let mut seq = serializer.serialize_seq(Some(values.len()))?;
                for value in values {
                    seq.serialize_element(value)?;
                }
                seq.end()
            }

            LoraValue::Map(map) => {
                let mut ser_map = serializer.serialize_map(Some(map.len()))?;
                for (k, v) in map {
                    ser_map.serialize_entry(k, v)?;
                }
                ser_map.end()
            }

            // These should ideally not reach output anymore if executor hydrates first.
            LoraValue::Node(id) => {
                let mut ser_map = serializer.serialize_map(Some(2))?;
                ser_map.serialize_entry("kind", "node")?;
                ser_map.serialize_entry("id", id)?;
                ser_map.end()
            }

            LoraValue::Relationship(id) => {
                let mut ser_map = serializer.serialize_map(Some(2))?;
                ser_map.serialize_entry("kind", "relationship")?;
                ser_map.serialize_entry("id", id)?;
                ser_map.end()
            }

            LoraValue::Path(path) => {
                let mut ser_map = serializer.serialize_map(Some(3))?;
                ser_map.serialize_entry("kind", "path")?;
                ser_map.serialize_entry("nodes", &path.nodes)?;
                ser_map.serialize_entry("rels", &path.rels)?;
                ser_map.end()
            }

            LoraValue::Date(d) => serializer.serialize_str(&d.to_string()),
            LoraValue::Time(t) => serializer.serialize_str(&t.to_string()),
            LoraValue::LocalTime(t) => serializer.serialize_str(&t.to_string()),
            LoraValue::DateTime(dt) => serializer.serialize_str(&dt.to_string()),
            LoraValue::LocalDateTime(dt) => serializer.serialize_str(&dt.to_string()),
            LoraValue::Duration(dur) => serializer.serialize_str(&dur.to_string()),
            LoraValue::Point(p) => {
                let len = if p.z.is_some() { 4 } else { 3 };
                let mut m = serializer.serialize_map(Some(len))?;
                m.serialize_entry("srid", &p.srid)?;
                m.serialize_entry("x", &p.x)?;
                m.serialize_entry("y", &p.y)?;
                if let Some(z) = p.z {
                    m.serialize_entry("z", &z)?;
                }
                m.end()
            }
            LoraValue::Vector(v) => serialize_vector(serializer, v),
        }
    }
}

fn serialize_binary<S: Serializer>(serializer: S, v: &LoraBinary) -> Result<S::Ok, S::Error> {
    let mut m = serializer.serialize_map(Some(3))?;
    m.serialize_entry("kind", "binary")?;
    m.serialize_entry("length", &v.len())?;
    m.serialize_entry("segments", v.segments())?;
    m.end()
}

fn serialize_vector<S: Serializer>(serializer: S, v: &LoraVector) -> Result<S::Ok, S::Error> {
    let mut m = serializer.serialize_map(Some(4))?;
    m.serialize_entry("kind", "vector")?;
    m.serialize_entry("dimension", &v.dimension)?;
    m.serialize_entry("coordinateType", v.coordinate_type().as_str())?;
    // Render values using the narrowest numeric type that fits the
    // storage so downstream consumers (serde_json in particular) can
    // surface integers vs. floats without losing information.
    match &v.values {
        VectorValues::Float64(values) => m.serialize_entry("values", values)?,
        VectorValues::Float32(values) => {
            let widened: Vec<f64> = values.iter().map(|x| *x as f64).collect();
            m.serialize_entry("values", &widened)?;
        }
        VectorValues::Integer64(values) => m.serialize_entry("values", values)?,
        VectorValues::Integer32(values) => {
            let widened: Vec<i64> = values.iter().map(|x| *x as i64).collect();
            m.serialize_entry("values", &widened)?;
        }
        VectorValues::Integer16(values) => {
            let widened: Vec<i64> = values.iter().map(|x| *x as i64).collect();
            m.serialize_entry("values", &widened)?;
        }
        VectorValues::Integer8(values) => {
            let widened: Vec<i64> = values.iter().map(|x| *x as i64).collect();
            m.serialize_entry("values", &widened)?;
        }
    }
    m.end()
}

impl From<PropertyValue> for LoraValue {
    fn from(value: PropertyValue) -> Self {
        match value {
            PropertyValue::Null => LoraValue::Null,
            PropertyValue::Bool(v) => LoraValue::Bool(v),
            PropertyValue::Int(v) => LoraValue::Int(v),
            PropertyValue::Float(v) => LoraValue::Float(v),
            PropertyValue::String(v) => LoraValue::String(v),
            PropertyValue::Binary(v) => LoraValue::Binary(v),
            PropertyValue::List(values) => {
                LoraValue::List(values.into_iter().map(LoraValue::from).collect())
            }
            PropertyValue::Map(map) => LoraValue::Map(
                map.into_iter()
                    .map(|(k, v)| (k, LoraValue::from(v)))
                    .collect(),
            ),
            PropertyValue::Date(d) => LoraValue::Date(d),
            PropertyValue::Time(t) => LoraValue::Time(t),
            PropertyValue::LocalTime(t) => LoraValue::LocalTime(t),
            PropertyValue::DateTime(dt) => LoraValue::DateTime(dt),
            PropertyValue::LocalDateTime(dt) => LoraValue::LocalDateTime(dt),
            PropertyValue::Duration(dur) => LoraValue::Duration(dur),
            PropertyValue::Point(p) => LoraValue::Point(p),
            PropertyValue::Vector(v) => LoraValue::Vector(v),
        }
    }
}

/// Build a `LoraValue` from a borrowed `PropertyValue` in a single walk. Lets
/// callers that already hold `&PropertyValue` (property lookups on borrowed
/// records) skip the `prop.clone().into()` double-traversal.
impl From<&PropertyValue> for LoraValue {
    fn from(value: &PropertyValue) -> Self {
        match value {
            PropertyValue::Null => LoraValue::Null,
            PropertyValue::Bool(v) => LoraValue::Bool(*v),
            PropertyValue::Int(v) => LoraValue::Int(*v),
            PropertyValue::Float(v) => LoraValue::Float(*v),
            PropertyValue::String(v) => LoraValue::String(v.clone()),
            PropertyValue::Binary(v) => LoraValue::Binary(v.clone()),
            PropertyValue::List(values) => {
                LoraValue::List(values.iter().map(LoraValue::from).collect())
            }
            PropertyValue::Map(map) => LoraValue::Map(
                map.iter()
                    .map(|(k, v)| (k.clone(), LoraValue::from(v)))
                    .collect(),
            ),
            PropertyValue::Date(d) => LoraValue::Date(d.clone()),
            PropertyValue::Time(t) => LoraValue::Time(t.clone()),
            PropertyValue::LocalTime(t) => LoraValue::LocalTime(t.clone()),
            PropertyValue::DateTime(dt) => LoraValue::DateTime(dt.clone()),
            PropertyValue::LocalDateTime(dt) => LoraValue::LocalDateTime(dt.clone()),
            PropertyValue::Duration(dur) => LoraValue::Duration(dur.clone()),
            PropertyValue::Point(p) => LoraValue::Point(p.clone()),
            PropertyValue::Vector(v) => LoraValue::Vector(v.clone()),
        }
    }
}

impl From<LoraValue> for PropertyValue {
    fn from(value: LoraValue) -> Self {
        match value {
            LoraValue::Null => PropertyValue::Null,
            LoraValue::Bool(v) => PropertyValue::Bool(v),
            LoraValue::Int(v) => PropertyValue::Int(v),
            LoraValue::Float(v) => PropertyValue::Float(v),
            LoraValue::String(v) => PropertyValue::String(v),
            LoraValue::Binary(v) => PropertyValue::Binary(v),
            LoraValue::List(values) => {
                PropertyValue::List(values.into_iter().map(PropertyValue::from).collect())
            }
            LoraValue::Map(map) => PropertyValue::Map(
                map.into_iter()
                    .map(|(k, v)| (k, PropertyValue::from(v)))
                    .collect(),
            ),
            LoraValue::Node(id) => PropertyValue::String(format!("node:{id}")),
            LoraValue::Relationship(id) => PropertyValue::String(format!("rel:{id}")),
            LoraValue::Path(_) => PropertyValue::Null,
            LoraValue::Date(d) => PropertyValue::Date(d),
            LoraValue::Time(t) => PropertyValue::Time(t),
            LoraValue::LocalTime(t) => PropertyValue::LocalTime(t),
            LoraValue::DateTime(dt) => PropertyValue::DateTime(dt),
            LoraValue::LocalDateTime(dt) => PropertyValue::LocalDateTime(dt),
            LoraValue::Duration(dur) => PropertyValue::Duration(dur),
            LoraValue::Point(p) => PropertyValue::Point(p),
            LoraValue::Vector(v) => PropertyValue::Vector(v),
        }
    }
}

/// Errors that can arise when converting a `LoraValue` into a
/// `PropertyValue` for storage on a node or relationship.
#[derive(Debug, Clone, PartialEq)]
pub enum PropertyConversionError {
    /// A list entry contained a VECTOR value. Vectors are first-class
    /// properties themselves but they cannot be nested inside lists.
    NestedVectorInList,
    /// Produced when something that cannot appear on disk (e.g. a `Path`
    /// value captured by mistake) is asked to be converted — surfaced so
    /// callers can reject it instead of silently stringifying.
    #[allow(dead_code)]
    UnsupportedKind(&'static str),
}

impl std::fmt::Display for PropertyConversionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PropertyConversionError::NestedVectorInList => {
                write!(f, "lists stored as properties cannot contain VECTOR values")
            }
            PropertyConversionError::UnsupportedKind(kind) => {
                write!(f, "cannot store {kind} as a property")
            }
        }
    }
}

impl std::error::Error for PropertyConversionError {}

/// Fallible conversion used on every write path
/// (`set_property_from_expr`, `overwrite_entity_target`,
/// `mutate_entity_target`, `eval_properties_expr`, plus CREATE /
/// MERGE). Rejects VECTOR values nested inside lists at any depth —
/// everything else falls through to the infallible `From`
/// implementation above. A top-level VECTOR property is always fine;
/// only LISTs that directly contain a VECTOR entry are rejected.
pub fn lora_value_to_property(value: LoraValue) -> Result<PropertyValue, PropertyConversionError> {
    /// Visit every nested value and, whenever we cross a `List`, flag the
    /// `Vector` entries it directly contains. We still recurse through
    /// `Map` and other `List` values so a vector buried under
    /// `{inner: [vector(...)]}` is caught too.
    fn visit(value: &LoraValue, inside_list: bool) -> Result<(), PropertyConversionError> {
        match value {
            LoraValue::Vector(_) if inside_list => Err(PropertyConversionError::NestedVectorInList),
            LoraValue::List(items) => {
                for item in items {
                    visit(item, true)?;
                }
                Ok(())
            }
            LoraValue::Map(m) => {
                for v in m.values() {
                    visit(v, inside_list)?;
                }
                Ok(())
            }
            _ => Ok(()),
        }
    }

    visit(&value, false)?;
    Ok(PropertyValue::from(value))
}

/// A row slot's value. Large values sit behind an `Arc`, so cloning the row
/// (every expand candidate, OPTIONAL MATCH merge, UNWIND element, FOREACH
/// iteration) bumps a refcount instead of copying a list the row only carries
/// (E-4: a 10k-element list made a 200-row hop ~250x slower). Small values
/// stay inline: an `Arc` in every slot costs an allocation per insert and a
/// pointer chase per read, 10-16% on hot paths.
#[derive(Debug, Clone)]
enum SlotValue {
    Inline(LoraValue),
    Shared(Arc<LoraValue>),
}

impl SlotValue {
    #[inline]
    fn new(value: LoraValue) -> Self {
        // Ids, scalars and strings, most of what rows hold, are decided
        // here; only containers walk `is_large`.
        let large = match &value {
            LoraValue::String(s) => s.len() >= LARGE_BYTES,
            LoraValue::List(_)
            | LoraValue::Map(_)
            | LoraValue::Path(_)
            | LoraValue::Binary(_)
            | LoraValue::Vector(_) => is_large(&value),
            _ => false,
        };
        if large {
            SlotValue::Shared(Arc::new(value))
        } else {
            SlotValue::Inline(value)
        }
    }

    #[inline]
    fn get(&self) -> &LoraValue {
        match self {
            SlotValue::Inline(v) => v,
            SlotValue::Shared(v) => v,
        }
    }

    #[inline]
    fn into_value(self) -> LoraValue {
        match self {
            SlotValue::Inline(v) => v,
            SlotValue::Shared(v) => Arc::try_unwrap(v).unwrap_or_else(|v| (*v).clone()),
        }
    }
}

/// A string at least this long is shared rather than copied.
const LARGE_BYTES: usize = 256;

impl PartialEq for SlotValue {
    fn eq(&self, other: &Self) -> bool {
        self.get() == other.get()
    }
}

/// Whether copying `value` costs enough for a row to share it: a list, map
/// or path with 8+ entries or a nested container, a long string, a binary or
/// a vector. Looks at no more than 8 elements.
fn is_large(value: &LoraValue) -> bool {
    const ENTRIES: usize = 8;
    let heavy = |v: &LoraValue| match v {
        LoraValue::List(_)
        | LoraValue::Map(_)
        | LoraValue::Path(_)
        | LoraValue::Vector(_)
        | LoraValue::Binary(_) => true,
        LoraValue::String(s) => s.len() >= LARGE_BYTES,
        _ => false,
    };
    match value {
        LoraValue::List(items) => items.len() >= ENTRIES || items.iter().any(heavy),
        LoraValue::Map(map) => map.len() >= ENTRIES || map.values().any(heavy),
        LoraValue::String(s) => s.len() >= LARGE_BYTES,
        LoraValue::Path(p) => p.nodes.len() >= ENTRIES,
        LoraValue::Binary(_) | LoraValue::Vector(_) => true,
        _ => false,
    }
}

#[derive(Debug, Clone, PartialEq)]
struct RowEntry {
    /// Stored alongside the value so iterators can hand back `&VarId` while
    /// the slot's position in `entries` remains the source of truth for
    /// lookups.
    var: VarId,
    /// `None` means "use the fallback `_{key}` lazily". This avoids allocating
    /// a String for every anonymous variable on the insert hot path.
    ///
    /// Shared `Arc<str>` rather than `String`: column names are fixed per
    /// operator, so producers mint one `Arc` per column and every row (and
    /// every row clone) holds a refcount instead of a private heap copy.
    name: Option<Arc<str>>,
    value: SlotValue,
}

/// Row layout: a positional vector indexed by `VarId.0`. Two reasons this beats
/// the previous `BTreeMap<VarId, RowEntry>`:
///
/// 1. **Cheaper clone.** Per-row clone is on the hottest path of the executor
///    (every filter, projection, expand, optional-match). A `BTreeMap` clone
///    allocates one tree node per entry; a `SmallVec` clone is a single
///    `memcpy` (or zero allocations when the row fits inline).
/// 2. **O(1) lookup.** `VarId`s are dense `u32`s minted from 0 by the
///    analyzer's `SymbolTable` (lora-analyzer/src/symbols.rs:23), so the
///    positional index is exact and tight.
///
/// `entries[i] == None` means "VarId(i) is unset"; the cached `len_set`
/// counter keeps `len()` O(1) without scanning. The inline capacity (`8`)
/// covers typical query rows without touching the heap.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Row {
    entries: smallvec::SmallVec<Option<RowEntry>, 8>,
    len_set: u32,
}

impl Serialize for Row {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut ser_map = serializer.serialize_map(Some(self.len()))?;
        for entry in self.entries.iter().flatten() {
            match &entry.name {
                Some(name) => ser_map.serialize_entry(&**name, entry.value.get())?,
                None => {
                    let fallback = format!("_{}", entry.var);
                    ser_map.serialize_entry(fallback.as_str(), entry.value.get())?;
                }
            }
        }
        ser_map.end()
    }
}

impl Row {
    pub fn new() -> Self {
        Self::default()
    }

    #[inline]
    pub fn get(&self, key: VarId) -> Option<&LoraValue> {
        self.slot(key).map(SlotValue::get)
    }

    /// Returns the column name for `key`, generating the `_{key}` fallback
    /// on demand for entries inserted without an explicit name.
    pub fn get_name(&self, key: VarId) -> Option<String> {
        self.entries
            .get(key.0 as usize)
            .and_then(|slot| slot.as_ref())
            .map(|entry| match &entry.name {
                Some(n) => n.to_string(),
                None => format!("_{}", entry.var),
            })
    }

    #[inline]
    pub fn insert(&mut self, key: VarId, value: LoraValue) {
        self.set_value(key, SlotValue::new(value));
    }

    #[inline]
    pub fn insert_named(&mut self, key: VarId, name: impl Into<Arc<str>>, value: LoraValue) {
        self.set_named(key, name.into(), SlotValue::new(value));
    }

    /// [`Self::insert`] that never shares the value: for a binding rebound
    /// per element of a list construct (`reduce`, a comprehension or
    /// quantifier variable), where the row is reused rather than cloned.
    #[inline]
    pub fn insert_inline(&mut self, key: VarId, value: LoraValue) {
        self.set_value(key, SlotValue::Inline(value));
    }

    /// [`Self::insert_named`] that never shares the value: for a row about to
    /// leave the executor (hydrated output), where no clone follows and the
    /// `Arc` would be an allocation per row for nothing.
    pub fn insert_named_inline(&mut self, key: VarId, name: impl Into<Arc<str>>, value: LoraValue) {
        self.set_named(key, name.into(), SlotValue::Inline(value));
    }

    /// Bind `key` (named `name`) to the value `source` holds for `from`,
    /// sharing a large value instead of copying it. Returns `false`, and
    /// changes nothing, when `from` is unset in `source`.
    pub fn insert_named_from(
        &mut self,
        key: VarId,
        name: impl Into<Arc<str>>,
        source: &Row,
        from: VarId,
    ) -> bool {
        let Some(value) = source.slot(from).cloned() else {
            return false;
        };
        self.set_named(key, name.into(), value);
        true
    }

    /// [`Self::insert_named_from`] with the row itself as the source.
    pub fn insert_named_from_self(
        &mut self,
        key: VarId,
        name: impl Into<Arc<str>>,
        from: VarId,
    ) -> bool {
        let Some(value) = self.slot(from).cloned() else {
            return false;
        };
        self.set_named(key, name.into(), value);
        true
    }

    #[inline]
    fn slot(&self, key: VarId) -> Option<&SlotValue> {
        self.entries
            .get(key.0 as usize)
            .and_then(|slot| slot.as_ref())
            .map(|entry| &entry.value)
    }

    /// Set `key`'s value, keeping any explicit name already stored;
    /// otherwise leave the name as `None` so the fallback is produced lazily.
    #[inline]
    fn set_value(&mut self, key: VarId, value: SlotValue) {
        let idx = self.ensure_slot(key);
        match &mut self.entries[idx] {
            Some(existing) => existing.value = value,
            slot @ None => {
                *slot = Some(RowEntry {
                    var: key,
                    name: None,
                    value,
                });
                self.len_set += 1;
            }
        }
    }

    #[inline]
    fn set_named(&mut self, key: VarId, name: Arc<str>, value: SlotValue) {
        let idx = self.ensure_slot(key);
        let was_set = self.entries[idx].is_some();
        self.entries[idx] = Some(RowEntry {
            var: key,
            name: Some(name),
            value,
        });
        if !was_set {
            self.len_set += 1;
        }
    }

    pub fn extend_from(&mut self, other: &Row) {
        // Mirrors the previous `BTreeMap::insert` semantics: every set entry
        // in `other` overwrites the slot wholesale (name and value), no merge.
        for entry in other.entries.iter().flatten() {
            let idx = self.ensure_slot(entry.var);
            let was_set = self.entries[idx].is_some();
            self.entries[idx] = Some(entry.clone());
            if !was_set {
                self.len_set += 1;
            }
        }
    }

    /// Copy every entry of `other` whose variable is unset in `self`,
    /// keeping its name as stored (shared `Arc`, or none for anonymous
    /// variables) rather than materializing the `_{key}` fallback.
    pub fn fill_missing_from(&mut self, other: &Row) {
        for entry in other.entries.iter().flatten() {
            let idx = self.ensure_slot(entry.var);
            if self.entries[idx].is_none() {
                self.entries[idx] = Some(entry.clone());
                self.len_set += 1;
            }
        }
    }

    pub fn iter(&self) -> impl Iterator<Item = (&VarId, &LoraValue)> {
        self.entries
            .iter()
            .flatten()
            .map(|entry| (&entry.var, entry.value.get()))
    }

    /// Iterate `(key, name, value)`. The name is a `Cow`: borrowed when an
    /// explicit name was stored, and owned (lazily formatted as `_{key}`) for
    /// entries inserted via the anonymous `insert()` path.
    pub fn iter_named(
        &self,
    ) -> impl Iterator<Item = (&VarId, std::borrow::Cow<'_, str>, &LoraValue)> {
        self.entries.iter().flatten().map(|entry| {
            let name: std::borrow::Cow<'_, str> = match &entry.name {
                Some(n) => std::borrow::Cow::Borrowed(&**n),
                None => std::borrow::Cow::Owned(format!("_{}", entry.var)),
            };
            (&entry.var, name, entry.value.get())
        })
    }

    /// Consume the row and yield owned `(VarId, name, LoraValue)` triples.
    /// Used by hydrate_row to avoid cloning values on the projection hot path;
    /// names come back as the shared `Arc<str>` so re-inserting them into a
    /// new row costs a refcount, not an allocation.
    pub fn into_iter_named(self) -> impl Iterator<Item = (VarId, Arc<str>, LoraValue)> {
        self.entries.into_iter().flatten().map(|entry| {
            let RowEntry { var, name, value } = entry;
            (
                var,
                name.unwrap_or_else(|| Arc::from(format!("_{var}"))),
                value.into_value(),
            )
        })
    }

    pub fn len(&self) -> usize {
        self.len_set as usize
    }

    pub fn is_empty(&self) -> bool {
        self.len_set == 0
    }

    pub fn contains_key(&self, key: VarId) -> bool {
        self.entries
            .get(key.0 as usize)
            .is_some_and(|slot| slot.is_some())
    }

    /// Grow `entries` so that index `key.0` is in-range. Returns that index.
    /// New slots are filled with `None` (counted as unset by `len_set`).
    fn ensure_slot(&mut self, key: VarId) -> usize {
        let idx = key.0 as usize;
        if idx >= self.entries.len() {
            self.entries.resize_with(idx + 1, || None);
        }
        idx
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResultFormat {
    Rows,
    RowArrays,
    Graph,
    Combined,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ExecuteOptions {
    pub format: ResultFormat,
}

impl Default for ExecuteOptions {
    fn default() -> Self {
        Self {
            format: ResultFormat::Graph,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(untagged)]
pub enum QueryResult {
    Rows(RowsResult),
    RowArrays(RowArraysResult),
    Graph(GraphResult),
    Combined(CombinedResult),
}

#[derive(Debug, Clone, Serialize)]
pub struct RowsResult {
    pub rows: Vec<Row>,
}

#[derive(Debug, Clone, Serialize)]
pub struct RowArraysResult {
    pub columns: Vec<String>,
    pub rows: Vec<Vec<LoraValue>>,
}

#[derive(Debug, Clone, Serialize)]
pub struct GraphResult {
    pub graph: HydratedGraph,
}

#[derive(Debug, Clone, Serialize)]
pub struct CombinedResult {
    pub columns: Vec<String>,
    pub data: Vec<CombinedRow>,
    pub graph: HydratedGraph,
}

#[derive(Debug, Clone, Serialize)]
pub struct CombinedRow {
    pub row: Vec<LoraValue>,
}

#[derive(Debug, Clone, Serialize, Default)]
pub struct HydratedGraph {
    pub nodes: Vec<HydratedNode>,
    pub relationships: Vec<HydratedRelationship>,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct HydratedNode {
    pub id: i64,
    pub labels: Vec<String>,
    pub properties: BTreeMap<String, LoraValue>,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct HydratedRelationship {
    pub id: i64,
    #[serde(rename = "startId")]
    pub start_id: i64,
    #[serde(rename = "endId")]
    pub end_id: i64,
    #[serde(rename = "type")]
    pub rel_type: String,
    pub properties: BTreeMap<String, LoraValue>,
}

pub fn project_rows(rows: Vec<Row>, options: ExecuteOptions) -> QueryResult {
    match options.format {
        ResultFormat::Rows => QueryResult::Rows(RowsResult { rows }),

        ResultFormat::RowArrays => {
            let columns = infer_columns(&rows);
            let projected_rows = rows.iter().map(|row| row_to_array(row, &columns)).collect();

            QueryResult::RowArrays(RowArraysResult {
                columns,
                rows: projected_rows,
            })
        }

        ResultFormat::Graph => QueryResult::Graph(GraphResult {
            graph: collect_hydrated_graph(&rows),
        }),

        ResultFormat::Combined => {
            let columns = infer_columns(&rows);
            let data = rows
                .iter()
                .map(|row| CombinedRow {
                    row: row_to_array(row, &columns),
                })
                .collect();

            QueryResult::Combined(CombinedResult {
                columns,
                data,
                graph: collect_hydrated_graph(&rows),
            })
        }
    }
}

fn infer_columns(rows: &[Row]) -> Vec<String> {
    rows.first()
        .map(|row| {
            row.iter_named()
                .map(|(_, name, _)| name.into_owned())
                .collect::<Vec<_>>()
        })
        .unwrap_or_default()
}

fn row_to_array(row: &Row, columns: &[String]) -> Vec<LoraValue> {
    // Row entry count is small; a linear scan per column avoids allocating
    // owned names into an intermediate lookup map.
    columns
        .iter()
        .map(|col| {
            row.iter_named()
                .find(|(_, name, _)| name.as_ref() == col.as_str())
                .map(|(_, _, v)| v.clone())
                .unwrap_or(LoraValue::Null)
        })
        .collect()
}

fn collect_hydrated_graph(rows: &[Row]) -> HydratedGraph {
    let mut nodes = BTreeMap::<i64, HydratedNode>::new();
    let mut relationships = BTreeMap::<i64, HydratedRelationship>::new();

    for row in rows {
        for (_, _, value) in row.iter_named() {
            collect_graph_from_value(value, &mut nodes, &mut relationships);
        }
    }

    HydratedGraph {
        nodes: nodes.into_values().collect(),
        relationships: relationships.into_values().collect(),
    }
}

fn collect_graph_from_value(
    value: &LoraValue,
    nodes: &mut BTreeMap<i64, HydratedNode>,
    relationships: &mut BTreeMap<i64, HydratedRelationship>,
) {
    match value {
        LoraValue::List(values) => {
            for value in values {
                collect_graph_from_value(value, nodes, relationships);
            }
        }

        LoraValue::Map(map) => {
            if let Some(node) = try_as_hydrated_node(map) {
                nodes.entry(node.id).or_insert(node);
                return;
            }

            if let Some(rel) = try_as_hydrated_relationship(map) {
                relationships.entry(rel.id).or_insert(rel);
                return;
            }

            for value in map.values() {
                collect_graph_from_value(value, nodes, relationships);
            }
        }

        _ => {}
    }
}

fn try_as_hydrated_node(map: &BTreeMap<String, LoraValue>) -> Option<HydratedNode> {
    let id = match map.get("id")? {
        LoraValue::Int(v) => *v,
        _ => return None,
    };

    let labels = match map.get("labels")? {
        LoraValue::List(values) => values
            .iter()
            .map(|v| match v {
                LoraValue::String(s) => Some(s.clone()),
                _ => None,
            })
            .collect::<Option<Vec<_>>>()?,
        _ => return None,
    };

    let properties = match map.get("properties")? {
        LoraValue::Map(props) => props.clone(),
        _ => return None,
    };

    Some(HydratedNode {
        id,
        labels,
        properties,
    })
}

fn try_as_hydrated_relationship(map: &BTreeMap<String, LoraValue>) -> Option<HydratedRelationship> {
    match map.get("kind") {
        Some(LoraValue::String(kind)) if kind == "relationship" => {}
        _ => return None,
    }

    let id = match map.get("id")? {
        LoraValue::Int(v) => *v,
        _ => return None,
    };

    let start_id = match map.get("startId").or_else(|| map.get("src"))? {
        LoraValue::Int(v) => *v,
        _ => return None,
    };

    let end_id = match map.get("endId").or_else(|| map.get("dst"))? {
        LoraValue::Int(v) => *v,
        _ => return None,
    };

    let rel_type = match map.get("type")? {
        LoraValue::String(s) => s.clone(),
        _ => return None,
    };

    let properties = match map.get("properties")? {
        LoraValue::Map(props) => props.clone(),
        _ => return None,
    };

    Some(HydratedRelationship {
        id,
        start_id,
        end_id,
        rel_type,
        properties,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn big() -> LoraValue {
        LoraValue::List((0..100).map(LoraValue::Int).collect())
    }

    #[test]
    fn a_slot_is_no_larger_than_its_value() {
        // The `Shared` variant fits in `LoraValue`'s niche: rows stay as
        // compact as before, and clone as one memcpy for inline values.
        assert_eq!(
            std::mem::size_of::<SlotValue>(),
            std::mem::size_of::<LoraValue>()
        );
    }

    #[test]
    fn a_cloned_row_shares_a_large_value_and_copies_a_small_one() {
        let mut row = Row::new();
        row.insert(VarId(0), big());
        row.insert(VarId(1), LoraValue::List(vec![LoraValue::Int(1)]));
        let copy = row.clone();
        assert!(std::ptr::eq(
            row.get(VarId(0)).unwrap(),
            copy.get(VarId(0)).unwrap()
        ));
        assert!(!std::ptr::eq(
            row.get(VarId(1)).unwrap(),
            copy.get(VarId(1)).unwrap()
        ));
        assert_eq!(row, copy);
    }

    #[test]
    fn a_shared_value_is_handed_out_whole_by_each_owner() {
        let mut row = Row::new();
        row.insert_named(VarId(0), "big", big());
        let mut projected = Row::new();
        assert!(projected.insert_named_from(VarId(1), "alias", &row, VarId(0)));
        assert!(!projected.insert_named_from(VarId(2), "missing", &row, VarId(5)));
        assert!(std::ptr::eq(
            row.get(VarId(0)).unwrap(),
            projected.get(VarId(1)).unwrap()
        ));
        assert_eq!(projected.len(), 1);
        let (_, name, value) = projected.into_iter_named().next().unwrap();
        assert_eq!(&*name, "alias");
        assert_eq!(value, big());
        // The original still holds its own copy.
        assert_eq!(row.get(VarId(0)), Some(&big()));
    }
}
