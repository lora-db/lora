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

use std::collections::HashSet;
use std::fmt::Write as _;

use thiserror::Error;

use super::constraint_catalog::{
    ConstraintCatalog, ConstraintDefinition, StoredConstraintKind, StoredPropertyType,
    StoredPropertyTypeTerm, StoredScalarType, StoredVectorCoordType,
};
use super::index_catalog::StoredIndexEntity;
use super::InMemoryGraph;
use crate::types::{NodeId, Properties, PropertyValue, RelationshipId};

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

    fn validate_existing_nodes_for_constraint(
        &self,
        def: &ConstraintDefinition,
    ) -> Result<(), ConstraintViolation> {
        let label = def.label.as_str();
        let mut seen: HashSet<String> = HashSet::new();
        for (_, node) in self.iter_nodes() {
            if !node.labels.iter().any(|l| l == label) {
                continue;
            }
            validate_record_against_constraint(def, &node.properties, &mut seen)?;
        }
        Ok(())
    }

    fn validate_existing_rels_for_constraint(
        &self,
        def: &ConstraintDefinition,
    ) -> Result<(), ConstraintViolation> {
        let rel_type = def.label.as_str();
        let mut seen: HashSet<String> = HashSet::new();
        for (_, rel) in self.iter_rels() {
            if rel.rel_type != rel_type {
                continue;
            }
            validate_record_against_constraint(def, &rel.properties, &mut seen)?;
        }
        Ok(())
    }
}

fn validate_record_against_constraint(
    def: &ConstraintDefinition,
    properties: &Properties,
    seen: &mut HashSet<String>,
) -> Result<(), ConstraintViolation> {
    // 1) Existence checks.
    if def.kind.requires_existence() {
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

    // 3) Uniqueness checks. Only constraints that require uniqueness
    // run this; we still skip records that don't carry the full
    // property tuple (matches the "uniqueness only applies when
    // all constrained properties are present" rule for plain
    // uniqueness; key constraints always require existence so the
    // tuple is guaranteed present by step 1).
    if def.kind.requires_uniqueness() {
        let tuple_present = def
            .properties
            .iter()
            .all(|p| properties.contains_key(p.as_str()));
        if tuple_present {
            let key = property_tuple_key(def, properties);
            if !seen.insert(key) {
                let property_label = if def.properties.len() == 1 {
                    def.properties[0].clone()
                } else {
                    def.properties.join(", ")
                };
                return Err(ConstraintViolation::UniquenessViolated {
                    constraint: def.name.clone(),
                    entity: def.entity,
                    label: def.label.clone(),
                    property: property_label,
                });
            }
        }
    }

    Ok(())
}

/// Cheap stable string-encoding for a property tuple — sufficient as a
/// `HashSet` key in the per-constraint pre-create scan. Not exposed to
/// callers because the shape isn't durable.
fn property_tuple_key(def: &ConstraintDefinition, properties: &Properties) -> String {
    let mut out = String::with_capacity(64);
    for (i, key) in def.properties.iter().enumerate() {
        if i > 0 {
            out.push('\u{1f}');
        }
        if let Some(value) = properties.get(key.as_str()) {
            append_property_value_key(&mut out, value);
        }
    }
    out
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
    One(&'a str),
}

impl NodeLabelMatcher<'_> {
    fn contains(self, label: &str) -> bool {
        match self {
            NodeLabelMatcher::AnyOf(labels) => labels.iter().any(|l| l == label),
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
    for def in catalog.iter() {
        if !record.applies_to(def) {
            continue;
        }

        let properties = record.properties();
        let mut probe: HashSet<String> = HashSet::new();
        validate_record_against_constraint(def, properties, &mut probe)?;

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

fn any_other_node_with_tuple(
    graph: &InMemoryGraph,
    label: &str,
    keys: &[String],
    target: &[PropertyValue],
    skip: Option<NodeId>,
) -> bool {
    for (id, node) in graph.iter_nodes() {
        if Some(id) == skip {
            continue;
        }
        if !node.labels.iter().any(|l| l == label) {
            continue;
        }
        let matches = keys.iter().enumerate().all(|(idx, key)| {
            node.properties
                .get(key.as_str())
                .map(|v| v == &target[idx])
                .unwrap_or(false)
        });
        if matches {
            return true;
        }
    }
    false
}

fn any_other_rel_with_tuple(
    graph: &InMemoryGraph,
    rel_type: &str,
    keys: &[String],
    target: &[PropertyValue],
    skip: Option<RelationshipId>,
) -> bool {
    for (id, rel) in graph.iter_rels() {
        if Some(id) == skip {
            continue;
        }
        if rel.rel_type != rel_type {
            continue;
        }
        let matches = keys.iter().enumerate().all(|(idx, key)| {
            rel.properties
                .get(key.as_str())
                .map(|v| v == &target[idx])
                .unwrap_or(false)
        });
        if matches {
            return true;
        }
    }
    false
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
    properties: &Properties,
    key: &str,
    value: &PropertyValue,
) -> Option<Vec<PropertyValue>> {
    def.properties
        .iter()
        .map(|prop| {
            if prop == key {
                Some(value.clone())
            } else {
                properties.get(prop.as_str()).cloned()
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
        if !node.labels.iter().any(|l| l == &def.label) {
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
            if let Some(tuple) = constrained_tuple_after_set(def, &node.properties, key, value) {
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
        if !node.labels.iter().any(|l| l == &def.label) {
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
    let node = match graph.node_at(node_id) {
        Some(n) => n,
        None => return Ok(()),
    };
    check_record_constraints(
        catalog,
        graph,
        ConstraintRecord::Node {
            labels: NodeLabelMatcher::AnyOf(&node.labels),
            properties,
            skip: Some(node_id),
        },
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
        if def.label != rel.rel_type {
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
            if let Some(tuple) = constrained_tuple_after_set(def, &rel.properties, key, value) {
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
        if def.label != rel.rel_type {
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
    let rel = match graph.rel_at(rel_id) {
        Some(r) => r,
        None => return Ok(()),
    };
    check_record_constraints(
        catalog,
        graph,
        ConstraintRecord::Relationship {
            rel_type: &rel.rel_type,
            properties,
            skip: Some(rel_id),
        },
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
            properties: &node.properties,
            skip: Some(node_id),
        },
    )
}
