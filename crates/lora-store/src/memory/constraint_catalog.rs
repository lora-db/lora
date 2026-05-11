//! Catalog of explicitly-declared constraints (CREATE CONSTRAINT). Sibling
//! to [`super::index_catalog::IndexCatalog`]. Conflict detection mirrors
//! Neo4j's GQLSTATUS codes:
//!
//! - 22N65 — equivalent constraint exists (same kind + schema).
//! - 22N66 — conflicting constraint exists (same schema, different kind
//!   that cannot coexist — e.g. unique vs. node-key, or property-type
//!   constraints disagreeing on the target type).
//! - 22N67 — duplicated constraint name.
//! - 22N71 — duplicated index name (a constraint name colliding with an
//!   existing index name; the constraint's backing range index would
//!   need that name).
//! - 22N73 — constraint conflicts with existing index (uniqueness/key
//!   constraints require a backing range index, so an existing range
//!   index on the same schema would conflict).
//!
//! The store layer is responsible for plumbing the *backing index* for
//! uniqueness and key constraints; the catalog only records that one
//! is owned and stores its name (which equals the constraint name).

use std::collections::BTreeMap;
use std::fmt;

use serde::{Deserialize, Serialize};
use thiserror::Error;

use super::index_catalog::StoredIndexEntity;

#[derive(Debug, Default, Clone)]
pub struct ConstraintCatalog {
    by_name: BTreeMap<String, ConstraintDefinition>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ConstraintDefinition {
    pub name: String,
    pub kind: StoredConstraintKind,
    pub entity: StoredIndexEntity,
    pub label: String,
    pub properties: Vec<String>,
    /// Name of the backing RANGE index, if any. Same as `name` for
    /// uniqueness and key constraints; `None` for existence/type.
    pub owned_index: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum StoredConstraintKind {
    Unique,
    Existence,
    NodeKey,
    RelationshipKey,
    PropertyType(StoredPropertyType),
}

impl StoredConstraintKind {
    pub fn type_tag(&self, entity: StoredIndexEntity) -> &'static str {
        match (self, entity) {
            (StoredConstraintKind::Unique, StoredIndexEntity::Node) => "NODE_PROPERTY_UNIQUENESS",
            (StoredConstraintKind::Unique, StoredIndexEntity::Relationship) => {
                "RELATIONSHIP_PROPERTY_UNIQUENESS"
            }
            (StoredConstraintKind::Existence, StoredIndexEntity::Node) => "NODE_PROPERTY_EXISTENCE",
            (StoredConstraintKind::Existence, StoredIndexEntity::Relationship) => {
                "RELATIONSHIP_PROPERTY_EXISTENCE"
            }
            (StoredConstraintKind::NodeKey, _) => "NODE_KEY",
            (StoredConstraintKind::RelationshipKey, _) => "RELATIONSHIP_KEY",
            (StoredConstraintKind::PropertyType(_), StoredIndexEntity::Node) => {
                "NODE_PROPERTY_TYPE"
            }
            (StoredConstraintKind::PropertyType(_), StoredIndexEntity::Relationship) => {
                "RELATIONSHIP_PROPERTY_TYPE"
            }
        }
    }

    /// True for constraints that need a backing range index. Used by
    /// the store layer to decide whether to register/drop one alongside
    /// the catalog entry.
    pub fn requires_backing_index(&self) -> bool {
        matches!(
            self,
            StoredConstraintKind::Unique
                | StoredConstraintKind::NodeKey
                | StoredConstraintKind::RelationshipKey
        )
    }

    /// True for constraints that require every covered property to be
    /// present (existence semantics). Property uniqueness alone does
    /// NOT require all properties to exist.
    pub fn requires_existence(&self) -> bool {
        matches!(
            self,
            StoredConstraintKind::Existence
                | StoredConstraintKind::NodeKey
                | StoredConstraintKind::RelationshipKey
        )
    }

    /// True for constraints that enforce uniqueness across the full
    /// property tuple.
    pub fn requires_uniqueness(&self) -> bool {
        matches!(
            self,
            StoredConstraintKind::Unique
                | StoredConstraintKind::NodeKey
                | StoredConstraintKind::RelationshipKey
        )
    }
}

/// Stored representation of a property-type constraint's target type.
/// Mirrors the AST `PropertyTypeExpr` but lives in the store layer so
/// the catalog has no AST dependency.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StoredPropertyType {
    pub alternatives: Vec<StoredPropertyTypeTerm>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum StoredPropertyTypeTerm {
    Scalar(StoredScalarType),
    List {
        inner: Box<StoredPropertyTypeTerm>,
        not_null: bool,
    },
    Vector {
        coord: StoredVectorCoordType,
        dimension: u32,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum StoredScalarType {
    Boolean,
    String,
    Integer,
    Float,
    Date,
    LocalTime,
    ZonedTime,
    LocalDateTime,
    ZonedDateTime,
    Duration,
    Point,
    Map,
    Any,
}

impl StoredScalarType {
    pub const fn as_str(self) -> &'static str {
        match self {
            StoredScalarType::Boolean => "BOOLEAN",
            StoredScalarType::String => "STRING",
            StoredScalarType::Integer => "INTEGER",
            StoredScalarType::Float => "FLOAT",
            StoredScalarType::Date => "DATE",
            StoredScalarType::LocalTime => "LOCAL TIME",
            StoredScalarType::ZonedTime => "ZONED TIME",
            StoredScalarType::LocalDateTime => "LOCAL DATETIME",
            StoredScalarType::ZonedDateTime => "ZONED DATETIME",
            StoredScalarType::Duration => "DURATION",
            StoredScalarType::Point => "POINT",
            StoredScalarType::Map => "MAP",
            StoredScalarType::Any => "ANY",
        }
    }
}

impl fmt::Display for StoredScalarType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum StoredVectorCoordType {
    Int8,
    Int16,
    Int32,
    Int64,
    Float32,
    Float64,
}

impl StoredVectorCoordType {
    pub const fn as_str(self) -> &'static str {
        match self {
            StoredVectorCoordType::Int8 => "INT8",
            StoredVectorCoordType::Int16 => "INT16",
            StoredVectorCoordType::Int32 => "INT32",
            StoredVectorCoordType::Int64 => "INT64",
            StoredVectorCoordType::Float32 => "FLOAT32",
            StoredVectorCoordType::Float64 => "FLOAT64",
        }
    }
}

impl fmt::Display for StoredVectorCoordType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl fmt::Display for StoredPropertyType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (idx, term) in self.alternatives.iter().enumerate() {
            if idx > 0 {
                f.write_str(" | ")?;
            }
            write!(f, "{term}")?;
        }
        Ok(())
    }
}

impl fmt::Display for StoredPropertyTypeTerm {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            StoredPropertyTypeTerm::Scalar(scalar) => write!(f, "{scalar}"),
            StoredPropertyTypeTerm::List { inner, not_null } => {
                if *not_null {
                    write!(f, "LIST<{inner} NOT NULL>")
                } else {
                    write!(f, "LIST<{inner}>")
                }
            }
            StoredPropertyTypeTerm::Vector { coord, dimension } => {
                write!(f, "VECTOR<{coord}>({dimension})")
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ConstraintRequest {
    pub name: String,
    pub kind: StoredConstraintKind,
    pub entity: StoredIndexEntity,
    pub label: String,
    pub properties: Vec<String>,
}

#[derive(Debug, Clone)]
pub enum CreateConstraintOutcome {
    Created(ConstraintDefinition),
    NoOpExists(ConstraintDefinition),
}

#[derive(Debug, Clone, Error)]
pub enum CreateConstraintError {
    /// 22N65 — an equivalent constraint already exists.
    #[error("equivalent constraint already exists: {0}")]
    EquivalentConstraintExists(String),
    /// 22N66 — a conflicting constraint already exists on the same schema.
    #[error("conflicting constraint already exists: {0}")]
    ConflictingConstraint(String),
    /// 22N67 — a constraint with the same name (any kind) already exists.
    #[error("a constraint with the same name already exists: {0}")]
    DuplicateName(String),
    /// 22N71 — name collides with an existing index name.
    #[error("an index with the same name already exists: {0}")]
    DuplicateIndexName(String),
    /// 22N73 — uniqueness/key constraint would conflict with an existing
    /// range index on the same schema.
    #[error("constraint conflicts with existing index: {0}")]
    BackingIndexConflict(String),
    /// 22N90 — property type unsupported in constraint.
    #[error("property type unsupported in constraint: {0}")]
    UnsupportedPropertyType(String),
    /// 50N11 — existing data already violates the proposed constraint.
    /// Wraps the underlying [`crate::ConstraintViolation`] description.
    #[error("[50N11] constraint creation failed. {0}")]
    DataViolation(String),
    /// Backend doesn't support the catalog API.
    #[error("{0}")]
    Unsupported(&'static str),
}

impl CreateConstraintError {
    pub const fn gql_status(&self) -> &'static str {
        match self {
            CreateConstraintError::EquivalentConstraintExists(_) => "22N65",
            CreateConstraintError::ConflictingConstraint(_) => "22N66",
            CreateConstraintError::DuplicateName(_) => "22N67",
            CreateConstraintError::DuplicateIndexName(_) => "22N71",
            CreateConstraintError::BackingIndexConflict(_) => "22N73",
            CreateConstraintError::UnsupportedPropertyType(_) => "22N90",
            // Inner violation already carries its own 22N7x in the
            // wrapped message; the outer 50N11 is the "constraint
            // creation failed" envelope.
            CreateConstraintError::DataViolation(_) => "50N11",
            CreateConstraintError::Unsupported(_) => "0A000",
        }
    }
}

#[derive(Debug, Clone)]
pub enum DropConstraintOutcome {
    Dropped(ConstraintDefinition),
    NoOpMissing,
}

#[derive(Debug, Clone, Error)]
pub enum DropConstraintError {
    /// 42N51 — referenced constraint does not exist.
    #[error("no constraint named `{0}` exists in the catalog")]
    NotFound(String),
    #[error("{0}")]
    Unsupported(&'static str),
}

impl DropConstraintError {
    pub const fn gql_status(&self) -> &'static str {
        match self {
            DropConstraintError::NotFound(_) => "42N51",
            DropConstraintError::Unsupported(_) => "0A000",
        }
    }
}

impl ConstraintCatalog {
    pub fn list(&self) -> Vec<ConstraintDefinition> {
        self.by_name.values().cloned().collect()
    }

    pub fn get(&self, name: &str) -> Option<&ConstraintDefinition> {
        self.by_name.get(name)
    }

    pub fn contains_name(&self, name: &str) -> bool {
        self.by_name.contains_key(name)
    }

    pub fn constraint_owning_index(&self, index_name: &str) -> Option<&ConstraintDefinition> {
        self.by_name
            .values()
            .find(|def| def.owned_index.as_deref() == Some(index_name))
    }

    /// Iterate the catalog in stored order. Useful for enforcement scans
    /// that need to know which constraints apply to a (label, prop) tuple.
    pub fn iter(&self) -> impl Iterator<Item = &ConstraintDefinition> {
        self.by_name.values()
    }

    /// Looks for an exact-shape match (same kind, entity, label, props,
    /// and type for property-type constraints). Property order matters
    /// — `(a, b)` and `(b, a)` are different constraints.
    pub fn find_equivalent(&self, request: &ConstraintRequest) -> Option<&ConstraintDefinition> {
        self.by_name.values().find(|def| {
            def.kind == request.kind
                && def.entity == request.entity
                && def.label == request.label
                && def.properties == request.properties
        })
    }

    /// Looks for any constraint on the same (entity, label, properties)
    /// schema regardless of kind. Used for 22N66 conflict detection —
    /// e.g. uniqueness vs. node-key on the same schema.
    pub fn find_same_schema(&self, request: &ConstraintRequest) -> Option<&ConstraintDefinition> {
        self.by_name.values().find(|def| {
            def.entity == request.entity
                && def.label == request.label
                && def.properties == request.properties
        })
    }

    pub fn try_create(
        &mut self,
        request: ConstraintRequest,
        if_not_exists: bool,
    ) -> Result<CreateConstraintOutcome, CreateConstraintError> {
        // 1) Equivalent constraint (same shape) — IF NOT EXISTS swallows.
        if let Some(existing) = self.find_equivalent(&request) {
            let cloned = existing.clone();
            if if_not_exists {
                return Ok(CreateConstraintOutcome::NoOpExists(cloned));
            }
            return Err(CreateConstraintError::EquivalentConstraintExists(
                cloned.name,
            ));
        }

        // 2) Duplicate name (different shape, same name) — never swallowed,
        // matches Neo4j 22N67 precedence even under IF NOT EXISTS when the
        // existing entry doesn't match the requested shape.
        if let Some(existing) = self.by_name.get(&request.name) {
            let existing_clone = existing.clone();
            if if_not_exists {
                // The Neo4j docs treat same-name-but-different-shape as a
                // no-op notification under IF NOT EXISTS. Match that.
                return Ok(CreateConstraintOutcome::NoOpExists(existing_clone));
            }
            return Err(CreateConstraintError::DuplicateName(existing_clone.name));
        }

        // 3) Conflicting constraint (same schema, different kind that
        // cannot coexist). Uniqueness <-> NodeKey/RelKey is the canonical
        // case; type constraints on the same prop with different target
        // types also conflict.
        if let Some(existing) = self.find_same_schema(&request) {
            if constraint_kinds_conflict(&existing.kind, &request.kind) {
                if if_not_exists {
                    return Ok(CreateConstraintOutcome::NoOpExists(existing.clone()));
                }
                return Err(CreateConstraintError::ConflictingConstraint(
                    existing.name.clone(),
                ));
            }
        }

        let owned_index = request
            .kind
            .requires_backing_index()
            .then(|| request.name.clone());

        let def = ConstraintDefinition {
            name: request.name.clone(),
            kind: request.kind,
            entity: request.entity,
            label: request.label,
            properties: request.properties,
            owned_index,
        };
        self.by_name.insert(def.name.clone(), def.clone());
        Ok(CreateConstraintOutcome::Created(def))
    }

    pub fn try_drop(
        &mut self,
        name: &str,
        if_exists: bool,
    ) -> Result<DropConstraintOutcome, DropConstraintError> {
        match self.by_name.remove(name) {
            Some(def) => Ok(DropConstraintOutcome::Dropped(def)),
            None if if_exists => Ok(DropConstraintOutcome::NoOpMissing),
            None => Err(DropConstraintError::NotFound(name.to_string())),
        }
    }
}

/// Two constraint kinds on the same schema "conflict" when both cannot
/// coexist. The rules from the Neo4j reference:
///
/// - Uniqueness on the same schema as a key constraint (or vice-versa)
///   — both would back the same range index but with different
///   semantics. Forbidden.
/// - Two property-type constraints on the same property but with
///   different target types.
/// - Two of *the same kind* aren't a "conflict" here — that's covered
///   by the equivalent-constraint check upstream.
fn constraint_kinds_conflict(
    existing: &StoredConstraintKind,
    requested: &StoredConstraintKind,
) -> bool {
    kinds_conflict_for_validation(existing, requested)
}

/// Public-within-store variant so the graph-level register hook can run
/// the same conflict check without re-implementing the rules.
pub(super) fn kinds_conflict_for_validation(
    existing: &StoredConstraintKind,
    requested: &StoredConstraintKind,
) -> bool {
    use StoredConstraintKind::*;
    match (existing, requested) {
        // unique vs. key in either direction
        (Unique, NodeKey) | (NodeKey, Unique) => true,
        (Unique, RelationshipKey) | (RelationshipKey, Unique) => true,
        // property type with different target
        (PropertyType(a), PropertyType(b)) => a != b,
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn unique_req(name: &str, label: &str, props: &[&str]) -> ConstraintRequest {
        ConstraintRequest {
            name: name.into(),
            kind: StoredConstraintKind::Unique,
            entity: StoredIndexEntity::Node,
            label: label.into(),
            properties: props.iter().map(|s| s.to_string()).collect(),
        }
    }

    #[test]
    fn create_simple_unique() {
        let mut cat = ConstraintCatalog::default();
        let out = cat
            .try_create(unique_req("c1", "Book", &["isbn"]), false)
            .unwrap();
        assert!(matches!(out, CreateConstraintOutcome::Created(_)));
        assert_eq!(cat.list().len(), 1);
        let def = cat.get("c1").unwrap();
        assert_eq!(def.owned_index.as_deref(), Some("c1"));
    }

    #[test]
    fn duplicate_name_rejected() {
        let mut cat = ConstraintCatalog::default();
        cat.try_create(unique_req("c", "A", &["x"]), false).unwrap();
        let err = cat
            .try_create(unique_req("c", "B", &["y"]), false)
            .unwrap_err();
        assert!(matches!(err, CreateConstraintError::DuplicateName(_)));
        assert_eq!(err.gql_status(), "22N67");
    }

    #[test]
    fn equivalent_constraint_rejected() {
        let mut cat = ConstraintCatalog::default();
        cat.try_create(unique_req("c1", "Book", &["isbn"]), false)
            .unwrap();
        let err = cat
            .try_create(unique_req("c2", "Book", &["isbn"]), false)
            .unwrap_err();
        assert!(matches!(
            err,
            CreateConstraintError::EquivalentConstraintExists(_)
        ));
        assert_eq!(err.gql_status(), "22N65");
    }

    #[test]
    fn unique_vs_node_key_conflicts() {
        let mut cat = ConstraintCatalog::default();
        cat.try_create(unique_req("u", "Book", &["isbn"]), false)
            .unwrap();
        let mut nk = unique_req("nk", "Book", &["isbn"]);
        nk.kind = StoredConstraintKind::NodeKey;
        let err = cat.try_create(nk, false).unwrap_err();
        assert!(matches!(
            err,
            CreateConstraintError::ConflictingConstraint(_)
        ));
        assert_eq!(err.gql_status(), "22N66");
    }

    #[test]
    fn if_not_exists_no_op() {
        let mut cat = ConstraintCatalog::default();
        cat.try_create(unique_req("c1", "Book", &["isbn"]), false)
            .unwrap();
        let out = cat
            .try_create(unique_req("c1", "Book", &["isbn"]), true)
            .unwrap();
        assert!(matches!(out, CreateConstraintOutcome::NoOpExists(_)));
    }

    #[test]
    fn drop_existing() {
        let mut cat = ConstraintCatalog::default();
        cat.try_create(unique_req("c1", "Book", &["isbn"]), false)
            .unwrap();
        let out = cat.try_drop("c1", false).unwrap();
        assert!(matches!(out, DropConstraintOutcome::Dropped(_)));
        assert!(cat.list().is_empty());
    }

    #[test]
    fn drop_missing_if_exists() {
        let mut cat = ConstraintCatalog::default();
        let out = cat.try_drop("missing", true).unwrap();
        assert!(matches!(out, DropConstraintOutcome::NoOpMissing));
    }

    #[test]
    fn drop_missing_errors() {
        let mut cat = ConstraintCatalog::default();
        let err = cat.try_drop("missing", false).unwrap_err();
        assert!(matches!(err, DropConstraintError::NotFound(_)));
        assert_eq!(err.gql_status(), "42N51");
    }

    #[test]
    fn existence_does_not_back_index() {
        let mut cat = ConstraintCatalog::default();
        let req = ConstraintRequest {
            name: "e".into(),
            kind: StoredConstraintKind::Existence,
            entity: StoredIndexEntity::Node,
            label: "L".into(),
            properties: vec!["p".into()],
        };
        let out = cat.try_create(req, false).unwrap();
        let CreateConstraintOutcome::Created(def) = out else {
            panic!("expected Created");
        };
        assert!(def.owned_index.is_none());
    }

    #[test]
    fn different_property_type_conflicts() {
        let mut cat = ConstraintCatalog::default();
        let a = ConstraintRequest {
            name: "t1".into(),
            kind: StoredConstraintKind::PropertyType(StoredPropertyType {
                alternatives: vec![StoredPropertyTypeTerm::Scalar(StoredScalarType::Integer)],
            }),
            entity: StoredIndexEntity::Relationship,
            label: "PART_OF".into(),
            properties: vec!["order".into()],
        };
        cat.try_create(a, false).unwrap();
        let b = ConstraintRequest {
            name: "t2".into(),
            kind: StoredConstraintKind::PropertyType(StoredPropertyType {
                alternatives: vec![StoredPropertyTypeTerm::Scalar(StoredScalarType::Float)],
            }),
            entity: StoredIndexEntity::Relationship,
            label: "PART_OF".into(),
            properties: vec!["order".into()],
        };
        let err = cat.try_create(b, false).unwrap_err();
        assert!(matches!(
            err,
            CreateConstraintError::ConflictingConstraint(_)
        ));
    }
}
