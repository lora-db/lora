//! Constraint enforcement primitives.
//!
//! The [`ConstraintCatalog`](super::ConstraintCatalog) records *what*
//! constraints exist; this module knows *how* to check whether a piece
//! of data — either an existing record or a proposed mutation —
//! complies.
//!
//! Why it lives here: both the DDL pre-create scan ("does the current
//! graph already violate the constraint we're about to register?") and
//! the runtime mutation pre-check ("would this write violate any
//! installed constraint?") need the same value-shape inspection. The
//! executor calls the runtime hooks from `lora-executor`; the DDL path
//! calls the scan from [`super::graph::InMemoryGraph::register_constraint`].
//!
//! Performance: the storage impl checks an atomic active-constraint
//! counter before calling into this module, so workloads that never call
//! `CREATE CONSTRAINT` skip the catalog lock entirely. Once constraints
//! exist, the catalog read is held across validation of a single mutation;
//! the writer mutex on the database serialises it against concurrent DDL.

// Several helpers below are pub-within-store and used by the runtime
// pre-check the executor will install in the next iteration; suppress
// dead-code while that wiring is still pending.
#![allow(dead_code)]

use std::fmt::Write as _;
use std::hash::{BuildHasher, BuildHasherDefault};
use std::sync::Arc;

use thiserror::Error;

use super::constraint_catalog::{
    ConstraintCatalog, ConstraintDefinition, StoredConstraintKind, StoredPropertyType,
    StoredPropertyTypeTerm, StoredScalarType, StoredVectorCoordType,
};
use super::cow::KeyHasher;
use super::index_catalog::StoredIndexEntity;
use super::InMemoryGraph;
use crate::encoded::{StoredNode, StoredRel};
use crate::types::{NodeId, Properties, PropertyValue, RelationshipId};
use crate::{PropsRef, ValueRef};

/// Why a mutation was rejected by a constraint check. The codes match
/// the GQLSTATUS-style shapes:
///
/// * 22N77 — property presence verification failed.
/// * 22N78 — property type verification failed.
/// * 22N79 — property uniqueness constraint violated.
/// * 22N80 — index entry conflict (duplicate row found by the backing
///   index during a CREATE CONSTRAINT scan).
/// * 50N11 — generic "constraint creation failed" wrapper used at DDL
///   time to surface the underlying 22N7x error.
#[derive(Debug, Clone, Error)]
pub enum ConstraintViolation {
    #[error("[22N77] property presence verification failed. {kind} must have the property `{property}`",
            kind = entity_kind_label(*entity, label))]
    MissingProperty {
        constraint: String,
        entity: StoredIndexEntity,
        label: String,
        property: String,
    },
    #[error("[22N77] property presence verification failed. {kind} must have the properties: {properties}",
            kind = entity_kind_label(*entity, label),
            properties = properties.join(", "))]
    MissingPropertiesForKey {
        constraint: String,
        entity: StoredIndexEntity,
        label: String,
        properties: Vec<String>,
    },
    #[error("[22N78] property type verification failed. {kind} must have property `{property}` with value type {expected}",
            kind = entity_kind_label(*entity, label))]
    WrongPropertyType {
        constraint: String,
        entity: StoredIndexEntity,
        label: String,
        property: String,
        expected: String,
    },
    #[error("[22N79] property uniqueness constraint violated. {kind} already has property `{property}` with the supplied value",
            kind = entity_kind_label(*entity, label))]
    UniquenessViolated {
        constraint: String,
        entity: StoredIndexEntity,
        label: String,
        property: String,
    },
}

impl ConstraintViolation {
    pub const fn gql_status(&self) -> &'static str {
        match self {
            ConstraintViolation::MissingProperty { .. }
            | ConstraintViolation::MissingPropertiesForKey { .. } => "22N77",
            ConstraintViolation::WrongPropertyType { .. } => "22N78",
            ConstraintViolation::UniquenessViolated { .. } => "22N79",
        }
    }

    pub fn constraint_name(&self) -> &str {
        match self {
            ConstraintViolation::MissingProperty { constraint, .. }
            | ConstraintViolation::MissingPropertiesForKey { constraint, .. }
            | ConstraintViolation::WrongPropertyType { constraint, .. }
            | ConstraintViolation::UniquenessViolated { constraint, .. } => constraint,
        }
    }
}

fn entity_kind_label(entity: StoredIndexEntity, label: &str) -> String {
    match entity {
        StoredIndexEntity::Node => format!("NODE with label `{label}`"),
        StoredIndexEntity::Relationship => format!("RELATIONSHIP with type `{label}`"),
    }
}

impl InMemoryGraph {
    /// Check whether the *current* graph contains data that would
    /// violate `def` if it were registered. Called from
    /// `register_constraint` just before the catalog write commits.
    ///
    /// Returns the first violation we find; we don't enumerate all
    /// failures because a single rejection is enough to refuse the
    /// CREATE CONSTRAINT.
    pub(super) fn validate_existing_data_for_constraint(
        &self,
        def: &ConstraintDefinition,
    ) -> Result<(), ConstraintViolation> {
        match def.entity {
            StoredIndexEntity::Node => self.validate_existing_nodes_for_constraint(def),
            StoredIndexEntity::Relationship => self.validate_existing_rels_for_constraint(def),
        }
    }

    /// Scans the label's nodes only, and decodes only the constrained
    /// properties of each: copying every property of every node out of
    /// its stored form was most of the time a restart spent on a
    /// constraint. A constraint on one property, other than a type
    /// constraint, reads the value in place without copying it.
    fn validate_existing_nodes_for_constraint(
        &self,
        def: &ConstraintDefinition,
    ) -> Result<(), ConstraintViolation> {
        let Some(ids) = self.nodes_by_label.get(def.label.as_str()) else {
            return Ok(());
        };
        let keys = constrained_keys(def);
        let properties_of = |id| {
            self.node_at(id)
                .map(|node| constrained_properties(&keys, node.properties()))
        };
        let slots = self.nodes.len() as u64;
        match single_key(def) {
            Some(key) => {
                let key_id = self.dicts.keys.id_of(key);
                validate_scope(
                    def,
                    ids,
                    slots,
                    |id, buf| {
                        let Some(bytes) = self.nodes.get(id as usize) else {
                            return Ok(None);
                        };
                        let node = StoredNode::new(id, bytes, &self.dicts);
                        let value = key_id.and_then(|key_id| node.properties().get_id(key_id));
                        check_single(def, value, buf)
                    },
                    properties_of,
                )
            }
            None => validate_scope(
                def,
                ids,
                slots,
                |id, buf| check_record(def, properties_of(id), buf),
                properties_of,
            ),
        }
    }

    fn validate_existing_rels_for_constraint(
        &self,
        def: &ConstraintDefinition,
    ) -> Result<(), ConstraintViolation> {
        let Some(ids) = self.relationships_by_type.get(def.label.as_str()) else {
            return Ok(());
        };
        let keys = constrained_keys(def);
        let properties_of = |id| {
            self.rel_at(id)
                .map(|rel| constrained_properties(&keys, rel.properties()))
        };
        let slots = self.relationships.len() as u64;
        match single_key(def) {
            Some(key) => {
                let key_id = self.dicts.keys.id_of(key);
                validate_scope(
                    def,
                    ids,
                    slots,
                    |id, buf| {
                        let Some(bytes) = self.relationships.get(id as usize) else {
                            return Ok(None);
                        };
                        let rel = StoredRel::new(id, bytes, &self.dicts);
                        let value = key_id.and_then(|key_id| rel.properties().get_id(key_id));
                        check_single(def, value, buf)
                    },
                    properties_of,
                )
            }
            None => validate_scope(
                def,
                ids,
                slots,
                |id, buf| check_record(def, properties_of(id), buf),
                properties_of,
            ),
        }
    }
}

/// The one property a constraint other than a type constraint covers,
/// when it covers one: its records are checked without copying values.
fn single_key(def: &ConstraintDefinition) -> Option<&str> {
    match (&def.kind, def.properties.as_slice()) {
        (StoredConstraintKind::PropertyType(_), _) => None,
        (_, [key]) => Some(key.as_str()),
        _ => None,
    }
}

/// What a scan learns from one record: `Err` if it violates the
/// existence or type checks, else its tuple hash when uniqueness applies
/// to it (built in `buf`).
type Checked = Result<Option<u64>, ConstraintViolation>;

/// [`Checked`] for a record's constrained properties (`None`: no record).
fn check_record(
    def: &ConstraintDefinition,
    properties: Option<Properties>,
    buf: &mut String,
) -> Checked {
    let Some(properties) = properties else {
        return Ok(None);
    };
    validate_record_against_constraint(def, &properties, true)?;
    Ok(tuple_hash(def, &properties, buf))
}

/// [`check_record`] for a [`single_key`] constraint, from the stored value
/// read in place: the same verdict and the same tuple key.
fn check_single(
    def: &ConstraintDefinition,
    value: Option<ValueRef<'_>>,
    buf: &mut String,
) -> Checked {
    let Some(value) = value else {
        // The existence check's violation, as for a record without it.
        validate_record_against_constraint(def, &Properties::new(), true)?;
        return Ok(None);
    };
    if !def.kind.requires_uniqueness() {
        return Ok(None);
    }
    buf.clear();
    append_value_ref_key(buf, value);
    Ok(Some(
        BuildHasherDefault::<KeyHasher>::default().hash_one(buf.as_str()),
    ))
}

/// [`append_property_value_key`] for a value read in place: a scalar is
/// written without being copied out first.
fn append_value_ref_key(out: &mut String, value: ValueRef<'_>) {
    match value {
        ValueRef::Null => out.push('N'),
        ValueRef::Bool(b) => {
            out.push('B');
            out.push(if b { 'T' } else { 'F' });
        }
        ValueRef::Int(i) => {
            out.push('I');
            let _ = write!(out, "{i}");
        }
        ValueRef::Float(f) => {
            out.push('F');
            let _ = write!(out, "{f:?}");
        }
        ValueRef::String(s) => {
            out.push('S');
            append_len_prefixed_str(out, s);
        }
        ValueRef::Other(value) => append_property_value_key(out, &value.get()),
    }
}

/// Ids a scan of [`validate_scope`] takes per task.
const SCAN_IDS: u64 = 1 << 16;

/// What one task of [`validate_scope`] found in its ids.
struct ScannedRange {
    /// The first record the existence or type checks reject.
    rejected: Option<(u64, ConstraintViolation)>,
    /// `(tuple hash, id)` of each record the uniqueness check applies to.
    tuples: Vec<(u64, u64)>,
}

/// Check the records `ids` lists (all below `slots`) against `def`, on
/// every core when the build allows, and report the violation a scan in
/// id order would meet first: per record, the existence and type checks,
/// then uniqueness against the records before it. `check` checks one
/// record ([`Checked`]); `properties_of` reads a record's constrained
/// properties, to confirm records whose tuples hash alike.
fn validate_scope(
    def: &ConstraintDefinition,
    ids: &super::id_bitmap::IdBitmap,
    slots: u64,
    check: impl Fn(u64, &mut String) -> Checked + Sync,
    properties_of: impl Fn(u64) -> Option<Properties> + Sync,
) -> Result<(), ConstraintViolation> {
    let scan = |start: u64| {
        let mut range = ScannedRange {
            rejected: None,
            tuples: Vec::new(),
        };
        let mut buf = String::new();
        for id in ids.iter_from(start).take_while(|&id| id < start + SCAN_IDS) {
            match check(id, &mut buf) {
                Err(violation) => {
                    // Nothing after it in this range can be met first.
                    range.rejected = Some((id, violation));
                    break;
                }
                Ok(Some(hash)) => range.tuples.push((hash, id)),
                Ok(None) => {}
            }
        }
        range
    };
    let starts: Vec<u64> = (0..slots.div_ceil(SCAN_IDS))
        .map(|i| i * SCAN_IDS)
        .collect();
    #[cfg(all(feature = "parallel", not(target_arch = "wasm32")))]
    let ranges: Vec<ScannedRange> = {
        use rayon::prelude::*;
        starts.into_par_iter().map(scan).collect()
    };
    #[cfg(not(all(feature = "parallel", not(target_arch = "wasm32"))))]
    let ranges: Vec<ScannedRange> = starts.into_iter().map(scan).collect();

    let mut rejected: Option<(u64, ConstraintViolation)> = None;
    let mut tuples = Vec::new();
    for range in ranges {
        if let Some((id, violation)) = range.rejected {
            if rejected.as_ref().is_none_or(|(first, _)| id < *first) {
                rejected = Some((id, violation));
            }
        }
        tuples.extend(range.tuples);
    }
    let repeat = first_repeat(def, tuples, &properties_of);
    match (repeat, rejected) {
        // At one record the existence and type checks come first.
        (Some(repeat), Some((first, _))) if repeat < first => Err(uniqueness_violation(def)),
        (_, Some((_, violation))) => Err(violation),
        (Some(_), None) => Err(uniqueness_violation(def)),
        (None, None) => Ok(()),
    }
}

/// The hash of `properties`' tuple key (built in `key`), or `None` when
/// uniqueness does not apply to the record: plain uniqueness only applies
/// when every constrained property is present, and a key constraint's
/// existence check has already failed a record without them.
fn tuple_hash(
    def: &ConstraintDefinition,
    properties: &Properties,
    key: &mut String,
) -> Option<u64> {
    if !def.kind.requires_uniqueness()
        || !def
            .properties
            .iter()
            .all(|p| properties.contains_key(p.as_str()))
    {
        return None;
    }
    key.clear();
    write_property_tuple_key(key, def, properties);
    Some(BuildHasherDefault::<KeyHasher>::default().hash_one(key.as_str()))
}

/// The smallest id whose tuple an earlier record already has, given each
/// record's `(tuple hash, id)`. Holds hashes, not keys: a 1M-node scan
/// that kept the keys held about three times the backing index it was
/// about to build. Records with one hash are confirmed by rebuilding
/// their keys from `properties_of`, so a collision never reads as a
/// duplicate.
fn first_repeat(
    def: &ConstraintDefinition,
    mut tuples: Vec<(u64, u64)>,
    properties_of: impl Fn(u64) -> Option<Properties>,
) -> Option<u64> {
    #[cfg(all(feature = "parallel", not(target_arch = "wasm32")))]
    {
        use rayon::slice::ParallelSliceMut;
        tuples.par_sort_unstable();
    }
    #[cfg(not(all(feature = "parallel", not(target_arch = "wasm32"))))]
    tuples.sort_unstable();

    let key_of = |id: u64| {
        let mut key = String::new();
        if let Some(properties) = properties_of(id) {
            write_property_tuple_key(&mut key, def, &properties);
        }
        key
    };
    let mut first: Option<u64> = None;
    for run in tuples
        .chunk_by(|a, b| a.0 == b.0)
        .filter(|run| run.len() > 1)
    {
        // Ids ascend within a run: the first key equal to an earlier one
        // is the run's earliest repeat.
        let mut keys: Vec<String> = Vec::with_capacity(run.len());
        for &(_, id) in run {
            let key = key_of(id);
            if keys.contains(&key) {
                first = Some(first.map_or(id, |first| first.min(id)));
                break;
            }
            keys.push(key);
        }
    }
    first
}

/// The constraint's property keys, interned once for a scan.
fn constrained_keys(def: &ConstraintDefinition) -> Vec<Arc<str>> {
    def.properties
        .iter()
        .map(|key| crate::intern(key))
        .collect()
}

/// The constrained properties a record holds: all a constraint check
/// reads.
fn constrained_properties(keys: &[Arc<str>], stored: PropsRef<'_>) -> Properties {
    keys.iter()
        .filter_map(|key| Some((Arc::clone(key), stored.get(key)?.to_owned())))
        .collect()
}

fn validate_record_against_constraint(
    def: &ConstraintDefinition,
    properties: &Properties,
    check_existence: bool,
) -> Result<(), ConstraintViolation> {
    // 1) Existence checks.
    if check_existence && def.kind.requires_existence() {
        let missing: Vec<String> = def
            .properties
            .iter()
            .filter(|p| !properties.contains_key(p.as_str()))
            .cloned()
            .collect();
        if !missing.is_empty() {
            return Err(if def.properties.len() == 1 {
                ConstraintViolation::MissingProperty {
                    constraint: def.name.clone(),
                    entity: def.entity,
                    label: def.label.clone(),
                    property: def.properties[0].clone(),
                }
            } else {
                ConstraintViolation::MissingPropertiesForKey {
                    constraint: def.name.clone(),
                    entity: def.entity,
                    label: def.label.clone(),
                    properties: def.properties.clone(),
                }
            });
        }
    }

    // 2) Property type checks (single-property only by grammar).
    if let StoredConstraintKind::PropertyType(target) = &def.kind {
        let key = &def.properties[0];
        if let Some(value) = properties.get(key.as_str()) {
            if !value_matches_property_type(value, target) {
                return Err(ConstraintViolation::WrongPropertyType {
                    constraint: def.name.clone(),
                    entity: def.entity,
                    label: def.label.clone(),
                    property: key.clone(),
                    expected: target.to_string(),
                });
            }
        }
    }

    Ok(())
}

/// Cheap stable string-encoding for a property tuple, appended to `out`:
/// two tuples are equal for the pre-create uniqueness scan exactly when
/// their keys are. Not exposed to callers because the shape isn't durable.
fn write_property_tuple_key(out: &mut String, def: &ConstraintDefinition, properties: &Properties) {
    for (i, key) in def.properties.iter().enumerate() {
        if i > 0 {
            out.push('\u{1f}');
        }
        if let Some(value) = properties.get(key.as_str()) {
            append_property_value_key(out, value);
        }
    }
}

fn append_property_value_key(out: &mut String, value: &PropertyValue) {
    match value {
        PropertyValue::Null => out.push('N'),
        PropertyValue::Bool(b) => {
            out.push('B');
            out.push(if *b { 'T' } else { 'F' });
        }
        PropertyValue::Int(i) => {
            out.push('I');
            out.push_str(&i.to_string());
        }
        PropertyValue::Float(f) => {
            out.push('F');
            out.push_str(&format!("{f:?}"));
        }
        PropertyValue::String(s) => {
            out.push('S');
            append_len_prefixed_str(out, s);
        }
        PropertyValue::Date(d) => {
            out.push_str("D:");
            append_len_prefixed_str(out, &format!("{d:?}"));
        }
        PropertyValue::Time(t) => {
            out.push_str("T:");
            append_len_prefixed_str(out, &format!("{t:?}"));
        }
        PropertyValue::LocalTime(t) => {
            out.push_str("LT:");
            append_len_prefixed_str(out, &format!("{t:?}"));
        }
        PropertyValue::DateTime(dt) => {
            out.push_str("DT:");
            append_len_prefixed_str(out, &format!("{dt:?}"));
        }
        PropertyValue::LocalDateTime(dt) => {
            out.push_str("LDT:");
            append_len_prefixed_str(out, &format!("{dt:?}"));
        }
        PropertyValue::Duration(d) => {
            out.push_str("DUR:");
            append_len_prefixed_str(out, &format!("{d:?}"));
        }
        PropertyValue::Point(p) => {
            out.push_str("P:");
            append_len_prefixed_str(out, &format!("{p:?}"));
        }
        PropertyValue::Vector(v) => {
            out.push_str("V:");
            append_len_prefixed_str(out, &v.to_key_string());
        }
        PropertyValue::List(items) => {
            out.push('L');
            append_len(out, items.len());
            for item in items {
                append_property_value_key(out, item);
            }
        }
        PropertyValue::Map(entries) => {
            out.push('M');
            append_len(out, entries.len());
            for (k, v) in entries {
                append_len_prefixed_str(out, k);
                append_property_value_key(out, v);
            }
        }
        PropertyValue::Binary(b) => {
            out.push_str("BIN:");
            append_len(out, b.len());
            for segment in b.chunks() {
                for byte in segment {
                    let _ = write!(out, "{byte:02x}");
                }
            }
        }
    }
}

fn append_len(out: &mut String, len: usize) {
    out.push_str(&len.to_string());
    out.push(':');
}

fn append_len_prefixed_str(out: &mut String, value: &str) {
    append_len(out, value.len());
    out.push_str(value);
}

/// True when `value` satisfies any branch of the target type. Used by
/// both DDL-time scans and runtime mutation checks.
pub fn value_matches_property_type(value: &PropertyValue, target: &StoredPropertyType) -> bool {
    target
        .alternatives
        .iter()
        .any(|term| value_matches_term(value, term))
}

fn value_matches_term(value: &PropertyValue, term: &StoredPropertyTypeTerm) -> bool {
    match term {
        StoredPropertyTypeTerm::Scalar(scalar) => value_matches_scalar(value, *scalar),
        StoredPropertyTypeTerm::List { inner, not_null } => match value {
            PropertyValue::List(items) => items.iter().all(|item| {
                if matches!(item, PropertyValue::Null) {
                    !*not_null
                } else {
                    value_matches_term(item, inner)
                }
            }),
            _ => false,
        },
        StoredPropertyTypeTerm::Vector { coord, dimension } => match value {
            PropertyValue::Vector(v) => vector_matches(v, *coord, *dimension),
            _ => false,
        },
    }
}

fn value_matches_scalar(value: &PropertyValue, scalar: StoredScalarType) -> bool {
    match (value, scalar) {
        (PropertyValue::Bool(_), StoredScalarType::Boolean) => true,
        (PropertyValue::String(_), StoredScalarType::String) => true,
        (PropertyValue::Int(_), StoredScalarType::Integer) => true,
        (PropertyValue::Float(_), StoredScalarType::Float) => true,
        (PropertyValue::Date(_), StoredScalarType::Date) => true,
        (PropertyValue::Time(_), StoredScalarType::ZonedTime) => true,
        (PropertyValue::LocalTime(_), StoredScalarType::LocalTime) => true,
        (PropertyValue::DateTime(_), StoredScalarType::ZonedDateTime) => true,
        (PropertyValue::LocalDateTime(_), StoredScalarType::LocalDateTime) => true,
        (PropertyValue::Duration(_), StoredScalarType::Duration) => true,
        (PropertyValue::Point(_), StoredScalarType::Point) => true,
        // Map / Any are rejected at DDL time, so they should never appear
        // here; reaching this arm with one of them indicates an upstream
        // bug — fail closed.
        _ => false,
    }
}

fn vector_matches(
    vector: &crate::types::LoraVector,
    coord: StoredVectorCoordType,
    dimension: u32,
) -> bool {
    if vector.dimension != dimension as usize {
        return false;
    }
    use crate::types::VectorValues;
    use StoredVectorCoordType::*;
    matches!(
        (&vector.values, coord),
        (VectorValues::Float64(_), Float64)
            | (VectorValues::Float32(_), Float32)
            | (VectorValues::Integer64(_), Int64)
            | (VectorValues::Integer32(_), Int32)
            | (VectorValues::Integer16(_), Int16)
            | (VectorValues::Integer8(_), Int8)
    )
}

#[derive(Clone, Copy)]
enum NodeLabelMatcher<'a> {
    AnyOf(&'a [String]),
    Names(&'a [crate::Name]),
    One(&'a str),
}

impl NodeLabelMatcher<'_> {
    fn contains(self, label: &str) -> bool {
        match self {
            NodeLabelMatcher::AnyOf(labels) => labels.iter().any(|l| l == label),
            NodeLabelMatcher::Names(labels) => labels.iter().any(|l| l == label),
            NodeLabelMatcher::One(candidate) => candidate == label,
        }
    }
}

#[derive(Clone, Copy)]
enum ConstraintRecord<'a> {
    Node {
        labels: NodeLabelMatcher<'a>,
        properties: &'a Properties,
        skip: Option<NodeId>,
    },
    Relationship {
        rel_type: &'a str,
        properties: &'a Properties,
        skip: Option<RelationshipId>,
    },
}

impl<'a> ConstraintRecord<'a> {
    fn applies_to(self, def: &ConstraintDefinition) -> bool {
        match self {
            ConstraintRecord::Node { labels, .. } => {
                def.entity == StoredIndexEntity::Node && labels.contains(&def.label)
            }
            ConstraintRecord::Relationship { rel_type, .. } => {
                def.entity == StoredIndexEntity::Relationship && def.label == rel_type
            }
        }
    }

    fn properties(self) -> &'a Properties {
        match self {
            ConstraintRecord::Node { properties, .. }
            | ConstraintRecord::Relationship { properties, .. } => properties,
        }
    }

    fn has_uniqueness_conflict(
        self,
        graph: &InMemoryGraph,
        def: &ConstraintDefinition,
        tuple: &[PropertyValue],
    ) -> bool {
        match self {
            ConstraintRecord::Node { skip, .. } => {
                any_other_node_with_tuple(graph, &def.label, &def.properties, tuple, skip)
            }
            ConstraintRecord::Relationship { skip, .. } => {
                any_other_rel_with_tuple(graph, &def.label, &def.properties, tuple, skip)
            }
        }
    }
}

fn check_record_constraints(
    catalog: &ConstraintCatalog,
    graph: &InMemoryGraph,
    record: ConstraintRecord<'_>,
) -> Result<(), ConstraintViolation> {
    check_record_constraints_with(catalog, graph, record, true)
}

fn check_record_constraints_with(
    catalog: &ConstraintCatalog,
    graph: &InMemoryGraph,
    record: ConstraintRecord<'_>,
    check_existence: bool,
) -> Result<(), ConstraintViolation> {
    for def in catalog.iter() {
        if !record.applies_to(def) {
            continue;
        }

        let properties = record.properties();
        validate_record_against_constraint(def, properties, check_existence)?;

        if def.kind.requires_uniqueness() {
            if let Some(tuple) = constrained_tuple(def, properties) {
                if record.has_uniqueness_conflict(graph, def, &tuple) {
                    return Err(uniqueness_violation(def));
                }
            }
        }
    }
    Ok(())
}

/// Public read-side check used by mutation paths: given a proposed
/// node create (labels + properties), is it accepted by every
/// installed constraint? Cheap when no constraints are registered
/// (single `is_empty()` on the catalog).
pub(crate) fn check_node_create(
    catalog: &ConstraintCatalog,
    graph: &InMemoryGraph,
    labels: &[String],
    properties: &Properties,
) -> Result<(), ConstraintViolation> {
    check_record_constraints(
        catalog,
        graph,
        ConstraintRecord::Node {
            labels: NodeLabelMatcher::AnyOf(labels),
            properties,
            skip: None,
        },
    )
}

pub(crate) fn check_relationship_create(
    catalog: &ConstraintCatalog,
    graph: &InMemoryGraph,
    rel_type: &str,
    properties: &Properties,
) -> Result<(), ConstraintViolation> {
    check_record_constraints(
        catalog,
        graph,
        ConstraintRecord::Relationship {
            rel_type,
            properties,
            skip: None,
        },
    )
}

/// [`check_node_create`] without the existence checks, for a statement
/// that checks existence once it has finished (a later `SET` in the same
/// statement may still supply the property). Pair with
/// [`check_node_existence`].
pub(crate) fn check_node_create_deferred(
    catalog: &ConstraintCatalog,
    graph: &InMemoryGraph,
    labels: &[String],
    properties: &Properties,
) -> Result<(), ConstraintViolation> {
    check_record_constraints_with(
        catalog,
        graph,
        ConstraintRecord::Node {
            labels: NodeLabelMatcher::AnyOf(labels),
            properties,
            skip: None,
        },
        false,
    )
}

/// Relationship counterpart of [`check_node_create_deferred`].
pub(crate) fn check_relationship_create_deferred(
    catalog: &ConstraintCatalog,
    graph: &InMemoryGraph,
    rel_type: &str,
    properties: &Properties,
) -> Result<(), ConstraintViolation> {
    check_record_constraints_with(
        catalog,
        graph,
        ConstraintRecord::Relationship {
            rel_type,
            properties,
            skip: None,
        },
        false,
    )
}

/// Existence (and key) constraints on a node as it stands now. A node
/// that no longer exists passes.
pub(crate) fn check_node_existence(
    catalog: &ConstraintCatalog,
    graph: &InMemoryGraph,
    node_id: NodeId,
) -> Result<(), ConstraintViolation> {
    let Some(node) = graph.node_at(node_id) else {
        return Ok(());
    };
    for def in catalog.iter() {
        if def.entity != StoredIndexEntity::Node
            || !def.kind.requires_existence()
            || !node.labels().strs().any(|l| l == def.label)
        {
            continue;
        }
        if let Some(missing) = def
            .properties
            .iter()
            .find(|p| !node.properties().contains_key(p.as_str()))
        {
            return Err(missing_property_violation(def, missing));
        }
    }
    Ok(())
}

/// Relationship counterpart of [`check_node_existence`].
pub(crate) fn check_relationship_existence(
    catalog: &ConstraintCatalog,
    graph: &InMemoryGraph,
    rel_id: RelationshipId,
) -> Result<(), ConstraintViolation> {
    let Some(rel) = graph.rel_at(rel_id) else {
        return Ok(());
    };
    for def in catalog.iter() {
        if def.entity != StoredIndexEntity::Relationship
            || !def.kind.requires_existence()
            || rel.rel_type() != def.label
        {
            continue;
        }
        if let Some(missing) = def
            .properties
            .iter()
            .find(|p| !rel.properties().contains_key(p.as_str()))
        {
            return Err(missing_property_violation(def, missing));
        }
    }
    Ok(())
}

/// Whether another node with `label` already holds `target` for `keys`.
///
/// Uniqueness constraints own a backing range index, which keeps the
/// label-scoped hash index for the first key active. Looking the value up
/// there makes each check O(matches) instead of a scan of every node in
/// the graph, which made loading N constrained nodes O(N^2). When the
/// value has no index image (temporal, spatial, vector) or the index is
/// not active, fall back to scanning the label's nodes only.
fn any_other_node_with_tuple(
    graph: &InMemoryGraph,
    label: &str,
    keys: &[String],
    target: &[PropertyValue],
    skip: Option<NodeId>,
) -> bool {
    let tuple_matches = |node: crate::NodeRef<'_>| {
        node.has_label(label)
            && keys.iter().enumerate().all(|(idx, key)| {
                node.properties()
                    .get(key.as_str())
                    .is_some_and(|v| v == target[idx])
            })
    };
    let candidates = match (keys.first(), target.first()) {
        (Some(key), Some(value)) => graph.indexed_node_ids(label, key, value),
        _ => None,
    };
    let candidates =
        candidates.unwrap_or_else(|| crate::GraphStorage::node_ids_by_label(graph, label));
    candidates
        .into_iter()
        .filter(|id| Some(*id) != skip)
        .any(|id| graph.node_at(id).is_some_and(tuple_matches))
}

/// Relationship counterpart of [`any_other_node_with_tuple`].
fn any_other_rel_with_tuple(
    graph: &InMemoryGraph,
    rel_type: &str,
    keys: &[String],
    target: &[PropertyValue],
    skip: Option<RelationshipId>,
) -> bool {
    let tuple_matches = |rel: crate::RelRef<'_>| {
        rel.rel_type() == rel_type
            && keys.iter().enumerate().all(|(idx, key)| {
                rel.properties()
                    .get(key.as_str())
                    .is_some_and(|v| v == target[idx])
            })
    };
    let candidates = match (keys.first(), target.first()) {
        (Some(key), Some(value)) => graph.indexed_rel_ids(rel_type, key, value),
        _ => None,
    };
    let candidates =
        candidates.unwrap_or_else(|| crate::GraphStorage::rel_ids_by_type(graph, rel_type));
    candidates
        .into_iter()
        .filter(|id| Some(*id) != skip)
        .any(|id| graph.rel_at(id).is_some_and(tuple_matches))
}

fn render_constraint_property_label(def: &ConstraintDefinition) -> String {
    if def.properties.len() == 1 {
        def.properties[0].clone()
    } else {
        def.properties.join(", ")
    }
}

fn uniqueness_violation(def: &ConstraintDefinition) -> ConstraintViolation {
    ConstraintViolation::UniquenessViolated {
        constraint: def.name.clone(),
        entity: def.entity,
        label: def.label.clone(),
        property: render_constraint_property_label(def),
    }
}

fn missing_property_violation(def: &ConstraintDefinition, property: &str) -> ConstraintViolation {
    if def.properties.len() == 1 {
        ConstraintViolation::MissingProperty {
            constraint: def.name.clone(),
            entity: def.entity,
            label: def.label.clone(),
            property: property.to_string(),
        }
    } else {
        ConstraintViolation::MissingPropertiesForKey {
            constraint: def.name.clone(),
            entity: def.entity,
            label: def.label.clone(),
            properties: def.properties.clone(),
        }
    }
}

fn constrained_tuple(
    def: &ConstraintDefinition,
    properties: &Properties,
) -> Option<Vec<PropertyValue>> {
    def.properties
        .iter()
        .map(|p| properties.get(p.as_str()).cloned())
        .collect()
}

fn constrained_tuple_after_set(
    def: &ConstraintDefinition,
    properties: crate::PropsRef<'_>,
    key: &str,
    value: &PropertyValue,
) -> Option<Vec<PropertyValue>> {
    // Reads only the constrained properties of the stored record.
    def.properties
        .iter()
        .map(|prop| {
            if prop == key {
                Some(value.clone())
            } else {
                properties.get(prop.as_str()).map(|v| v.to_owned())
            }
        })
        .collect()
}

/// Mutation pre-check: about to `SET node.key = value`. Validates
/// every node-level constraint whose schema covers any of the node's
/// labels and any of its constrained properties touched by this write.
pub(crate) fn check_node_set_property(
    catalog: &ConstraintCatalog,
    graph: &InMemoryGraph,
    node_id: NodeId,
    key: &str,
    value: &PropertyValue,
) -> Result<(), ConstraintViolation> {
    let node = match graph.node_at(node_id) {
        Some(n) => n,
        None => return Ok(()), // mutation will fail downstream
    };
    for def in catalog.iter() {
        if def.entity != StoredIndexEntity::Node {
            continue;
        }
        if !node.labels().strs().any(|l| l == def.label) {
            continue;
        }
        if !def.properties.iter().any(|p| p == key) {
            continue;
        }
        // Type check on the new value.
        if let StoredConstraintKind::PropertyType(target) = &def.kind {
            if !value_matches_property_type(value, target) {
                return Err(ConstraintViolation::WrongPropertyType {
                    constraint: def.name.clone(),
                    entity: def.entity,
                    label: def.label.clone(),
                    property: key.to_string(),
                    expected: target.to_string(),
                });
            }
        }
        // Uniqueness: build the post-set tuple and search the rest of
        // the graph for an identical one.
        if def.kind.requires_uniqueness() {
            if let Some(tuple) = constrained_tuple_after_set(def, node.properties(), key, value) {
                if any_other_node_with_tuple(
                    graph,
                    &def.label,
                    &def.properties,
                    &tuple,
                    Some(node_id),
                ) {
                    return Err(uniqueness_violation(def));
                }
            }
        }
    }
    Ok(())
}

/// Mutation pre-check: about to `REMOVE node.key`. Rejects when an
/// existence / key constraint requires the property to remain present.
pub(crate) fn check_node_remove_property(
    catalog: &ConstraintCatalog,
    graph: &InMemoryGraph,
    node_id: NodeId,
    key: &str,
) -> Result<(), ConstraintViolation> {
    let node = match graph.node_at(node_id) {
        Some(n) => n,
        None => return Ok(()),
    };
    for def in catalog.iter() {
        if def.entity != StoredIndexEntity::Node {
            continue;
        }
        if !node.labels().strs().any(|l| l == def.label) {
            continue;
        }
        if !def.kind.requires_existence() {
            continue;
        }
        if !def.properties.iter().any(|p| p == key) {
            continue;
        }
        return Err(missing_property_violation(def, key));
    }
    Ok(())
}

/// Mutation pre-check: about to replace the full property map on a
/// node. Validate the final record shape, but skip the node itself
/// when checking uniqueness.
pub(crate) fn check_node_replace_properties(
    catalog: &ConstraintCatalog,
    graph: &InMemoryGraph,
    node_id: NodeId,
    properties: &Properties,
) -> Result<(), ConstraintViolation> {
    check_node_replace_properties_with(catalog, graph, node_id, properties, true)
}

/// [`check_node_replace_properties`] without the existence checks, for a
/// statement that checks existence once it has finished (`SET n = {…},
/// n.required = …`). Pair with [`check_node_existence`].
pub(crate) fn check_node_replace_properties_deferred(
    catalog: &ConstraintCatalog,
    graph: &InMemoryGraph,
    node_id: NodeId,
    properties: &Properties,
) -> Result<(), ConstraintViolation> {
    check_node_replace_properties_with(catalog, graph, node_id, properties, false)
}

fn check_node_replace_properties_with(
    catalog: &ConstraintCatalog,
    graph: &InMemoryGraph,
    node_id: NodeId,
    properties: &Properties,
    check_existence: bool,
) -> Result<(), ConstraintViolation> {
    let labels = match graph.node_at(node_id) {
        Some(node) => node.labels().to_owned(),
        None => return Ok(()),
    };
    check_record_constraints_with(
        catalog,
        graph,
        ConstraintRecord::Node {
            labels: NodeLabelMatcher::Names(&labels),
            properties,
            skip: Some(node_id),
        },
        check_existence,
    )
}

pub(crate) fn check_relationship_set_property(
    catalog: &ConstraintCatalog,
    graph: &InMemoryGraph,
    rel_id: RelationshipId,
    key: &str,
    value: &PropertyValue,
) -> Result<(), ConstraintViolation> {
    let rel = match graph.rel_at(rel_id) {
        Some(r) => r,
        None => return Ok(()),
    };
    for def in catalog.iter() {
        if def.entity != StoredIndexEntity::Relationship {
            continue;
        }
        if def.label != rel.rel_type() {
            continue;
        }
        if !def.properties.iter().any(|p| p == key) {
            continue;
        }
        if let StoredConstraintKind::PropertyType(target) = &def.kind {
            if !value_matches_property_type(value, target) {
                return Err(ConstraintViolation::WrongPropertyType {
                    constraint: def.name.clone(),
                    entity: def.entity,
                    label: def.label.clone(),
                    property: key.to_string(),
                    expected: target.to_string(),
                });
            }
        }
        if def.kind.requires_uniqueness() {
            if let Some(tuple) = constrained_tuple_after_set(def, rel.properties(), key, value) {
                if any_other_rel_with_tuple(
                    graph,
                    &def.label,
                    &def.properties,
                    &tuple,
                    Some(rel_id),
                ) {
                    return Err(uniqueness_violation(def));
                }
            }
        }
    }
    Ok(())
}

pub(crate) fn check_relationship_remove_property(
    catalog: &ConstraintCatalog,
    graph: &InMemoryGraph,
    rel_id: RelationshipId,
    key: &str,
) -> Result<(), ConstraintViolation> {
    let rel = match graph.rel_at(rel_id) {
        Some(r) => r,
        None => return Ok(()),
    };
    for def in catalog.iter() {
        if def.entity != StoredIndexEntity::Relationship {
            continue;
        }
        if def.label != rel.rel_type() {
            continue;
        }
        if !def.kind.requires_existence() {
            continue;
        }
        if !def.properties.iter().any(|p| p == key) {
            continue;
        }
        return Err(missing_property_violation(def, key));
    }
    Ok(())
}

/// Mutation pre-check: about to replace the full property map on a
/// relationship. Validate the final record shape, but skip the
/// relationship itself when checking uniqueness.
pub(crate) fn check_relationship_replace_properties(
    catalog: &ConstraintCatalog,
    graph: &InMemoryGraph,
    rel_id: RelationshipId,
    properties: &Properties,
) -> Result<(), ConstraintViolation> {
    check_relationship_replace_properties_with(catalog, graph, rel_id, properties, true)
}

/// Relationship counterpart of [`check_node_replace_properties_deferred`].
pub(crate) fn check_relationship_replace_properties_deferred(
    catalog: &ConstraintCatalog,
    graph: &InMemoryGraph,
    rel_id: RelationshipId,
    properties: &Properties,
) -> Result<(), ConstraintViolation> {
    check_relationship_replace_properties_with(catalog, graph, rel_id, properties, false)
}

fn check_relationship_replace_properties_with(
    catalog: &ConstraintCatalog,
    graph: &InMemoryGraph,
    rel_id: RelationshipId,
    properties: &Properties,
    check_existence: bool,
) -> Result<(), ConstraintViolation> {
    let rel = match graph.rel_at(rel_id) {
        Some(r) => r,
        None => return Ok(()),
    };
    check_record_constraints_with(
        catalog,
        graph,
        ConstraintRecord::Relationship {
            rel_type: rel.rel_type(),
            properties,
            skip: Some(rel_id),
        },
        check_existence,
    )
}

/// Mutation pre-check: about to `SET n:Label` (add label). All
/// existence / type / uniqueness constraints attached to `Label`
/// suddenly start applying to this node; if any of them is violated
/// the mutation is rejected.
pub(crate) fn check_node_add_label(
    catalog: &ConstraintCatalog,
    graph: &InMemoryGraph,
    node_id: NodeId,
    label: &str,
) -> Result<(), ConstraintViolation> {
    let node = match graph.node_at(node_id) {
        Some(n) => n,
        None => return Ok(()),
    };
    check_record_constraints(
        catalog,
        graph,
        ConstraintRecord::Node {
            labels: NodeLabelMatcher::One(label),
            properties: &node.properties().to_owned(),
            skip: Some(node_id),
        },
    )
}

/// [`check_node_add_label`] without the existence checks, for a statement
/// that checks existence once it has finished. Pair with
/// [`check_node_existence`].
pub(crate) fn check_node_add_label_deferred(
    catalog: &ConstraintCatalog,
    graph: &InMemoryGraph,
    node_id: NodeId,
    label: &str,
) -> Result<(), ConstraintViolation> {
    let Some(node) = graph.node_at(node_id) else {
        return Ok(());
    };
    check_record_constraints_with(
        catalog,
        graph,
        ConstraintRecord::Node {
            labels: NodeLabelMatcher::One(label),
            properties: &node.properties().to_owned(),
            skip: Some(node_id),
        },
        false,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::intern;

    fn unique_on(property: &str) -> ConstraintDefinition {
        ConstraintDefinition {
            backing: Default::default(),
            name: "u".into(),
            kind: StoredConstraintKind::Unique,
            entity: StoredIndexEntity::Node,
            label: "L".into(),
            properties: vec![property.into()],
            owned_index: Some("u".into()),
        }
    }

    fn with(property: &str, value: PropertyValue) -> Properties {
        let mut properties = Properties::new();
        properties.insert(intern(property), value);
        properties
    }

    /// Records 0..n carry `values[id]` under `k`, all under one forced
    /// hash, so every record after the first takes the collision path.
    fn repeat_among_colliding(values: &[PropertyValue]) -> Option<u64> {
        let def = unique_on("k");
        let records: Vec<Properties> = values.iter().map(|v| with("k", v.clone())).collect();
        let tuples = (0..records.len() as u64).map(|id| (7, id)).collect();
        first_repeat(&def, tuples, |id| records.get(id as usize).cloned())
    }

    #[test]
    fn a_shared_hash_is_not_a_duplicate() {
        // Int 1 and Float 1.0 stay distinct, as they did as key strings.
        let values = [
            PropertyValue::Int(1),
            PropertyValue::Float(1.0),
            PropertyValue::String("1".into()),
            PropertyValue::Int(2),
        ];
        assert_eq!(repeat_among_colliding(&values), None);
    }

    #[test]
    fn an_equal_tuple_behind_a_collision_is_found() {
        let values = [
            PropertyValue::Int(1),
            PropertyValue::Int(2),
            PropertyValue::Int(3),
            PropertyValue::Int(2),
        ];
        assert_eq!(repeat_among_colliding(&values), Some(3));
    }

    #[test]
    fn the_earliest_repeat_is_reported() {
        let def = unique_on("k");
        let records: Vec<Properties> = [5, 6, 6, 5]
            .iter()
            .map(|v| with("k", PropertyValue::Int(*v)))
            .collect();
        let mut key = String::new();
        let tuples = records
            .iter()
            .enumerate()
            .map(|(id, properties)| (tuple_hash(&def, properties, &mut key).unwrap(), id as u64))
            .collect();
        let repeat = first_repeat(&def, tuples, |id| records.get(id as usize).cloned());
        assert_eq!(repeat, Some(2));
    }

    #[test]
    fn records_without_the_tuple_are_skipped() {
        let def = unique_on("k");
        let mut key = String::new();
        assert_eq!(
            tuple_hash(&def, &with("other", PropertyValue::Int(1)), &mut key),
            None
        );
    }

    #[test]
    fn a_value_checked_in_place_gets_the_copied_values_verdict_and_key() {
        let values = [
            PropertyValue::Null,
            PropertyValue::Bool(true),
            PropertyValue::Int(-42),
            PropertyValue::Int(i64::MIN),
            PropertyValue::Float(1.0),
            PropertyValue::Float(-0.0),
            PropertyValue::Float(f64::NAN),
            PropertyValue::String(String::new()),
            PropertyValue::String("héllo".into()),
            PropertyValue::List(vec![
                PropertyValue::Int(1),
                PropertyValue::String("x".into()),
            ]),
            PropertyValue::Date(crate::LoraDate::new(2024, 2, 29).unwrap()),
        ];
        for kind in [
            StoredConstraintKind::Unique,
            StoredConstraintKind::NodeKey,
            StoredConstraintKind::Existence,
        ] {
            let def = ConstraintDefinition {
                kind: kind.clone(),
                ..unique_on("k")
            };
            for value in &values {
                let mut copied = String::new();
                let mut in_place = String::new();
                let properties = with("k", value.clone());
                let slow = check_record(&def, Some(properties), &mut copied).unwrap();
                let fast = check_single(&def, Some(ValueRef::from(value)), &mut in_place).unwrap();
                assert_eq!(fast, slow, "{kind:?} {value:?}");
            }
            let mut buf = String::new();
            let slow = check_record(&def, Some(Properties::new()), &mut buf);
            let fast = check_single(&def, None, &mut buf);
            assert_eq!(
                fast.map_err(|e| e.gql_status()),
                slow.map_err(|e| e.gql_status()),
                "{kind:?} missing"
            );
        }
    }
}
