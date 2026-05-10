//! Catalog of explicitly-declared indexes (CREATE INDEX). Keyed by
//! index name, with conflict detection that mirrors Cypher's
//! `equivalent index already exists` (22N70) and
//! `duplicated index name` (22N71) shapes.
//!
//! The catalog records *what* the user asked us to maintain; the
//! actual data structures (hash buckets, label indexes) live in
//! [`super::property_index`] and on [`super::graph::InMemoryGraph`].
//! For RANGE indexes the catalog is paired with the existing lazy
//! property-index activation: an explicitly-named RANGE index causes
//! the underlying property-index to be force-populated and pinned so
//! it never gets evicted by future eviction logic.
//!
//! TEXT and POINT indexes register in the catalog and activate their
//! dedicated trigram / spatial structures. The optimizer only targets
//! those physical operators when the matching catalog scope is online.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Debug, Default, Clone)]
pub struct IndexCatalog {
    by_name: BTreeMap<String, IndexDefinition>,
    auto_seq: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct IndexDefinition {
    pub name: String,
    pub kind: StoredIndexKind,
    pub entity: StoredIndexEntity,
    /// `Some(label_or_type)` for property indexes. For LOOKUP token
    /// indexes the label/type is the wildcard captured by `labels(n)`
    /// or `type(r)` and is recorded as `None`.
    pub label: Option<String>,
    pub properties: Vec<String>,
    pub options: BTreeMap<String, IndexConfigValue>,
    pub state: StoredIndexState,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum StoredIndexKind {
    Range,
    Text,
    Point,
    Lookup,
}

impl StoredIndexKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            StoredIndexKind::Range => "RANGE",
            StoredIndexKind::Text => "TEXT",
            StoredIndexKind::Point => "POINT",
            StoredIndexKind::Lookup => "LOOKUP",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum StoredIndexEntity {
    Node,
    Relationship,
}

impl StoredIndexEntity {
    pub const fn as_str(self) -> &'static str {
        match self {
            StoredIndexEntity::Node => "NODE",
            StoredIndexEntity::Relationship => "RELATIONSHIP",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum StoredIndexState {
    Online,
    Populating,
}

impl StoredIndexState {
    pub const fn as_str(self) -> &'static str {
        match self {
            StoredIndexState::Online => "ONLINE",
            StoredIndexState::Populating => "POPULATING",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum IndexConfigValue {
    Number(f64),
    Integer(i64),
    String(String),
    Bool(bool),
    List(Vec<IndexConfigValue>),
    Map(BTreeMap<String, IndexConfigValue>),
    Null,
}

#[derive(Debug, Clone)]
pub enum CreateIndexOutcome {
    /// A new index was registered.
    Created(IndexDefinition),
    /// The request was idempotent (`IF NOT EXISTS`) and matched an existing entry.
    NoOpExists(IndexDefinition),
}

#[derive(Debug, Clone, Error)]
pub enum CreateIndexError {
    /// 22N70 — an index with the same schema and kind already exists.
    #[error("equivalent index already exists: {}", format_index_schema(.0))]
    EquivalentIndexExists(IndexDefinition),
    /// 22N71 — an index with the same name (any kind) already exists.
    #[error("an index with the same name already exists: {}", .0.name)]
    DuplicateName(IndexDefinition),
    /// Backend doesn't support the catalog API.
    #[error("{0}")]
    Unsupported(&'static str),
}

impl CreateIndexError {
    /// GQLSTATUS-shaped code mapping. Used by the database layer to
    /// surface conflicts via the structured error result type so
    /// bindings can match on a stable string.
    pub const fn gql_status(&self) -> &'static str {
        match self {
            CreateIndexError::EquivalentIndexExists(_) => "22N70",
            CreateIndexError::DuplicateName(_) => "22N71",
            CreateIndexError::Unsupported(_) => "0A000",
        }
    }
}

#[derive(Debug, Clone)]
pub enum DropIndexOutcome {
    Dropped(IndexDefinition),
    /// `IF EXISTS` requested and no matching index was registered.
    NoOpMissing,
}

#[derive(Debug, Clone, Error)]
pub enum DropIndexError {
    /// 42N51 — referenced index does not exist.
    #[error("no index named `{0}` exists in the catalog")]
    NotFound(String),
    #[error("{0}")]
    Unsupported(&'static str),
}

impl DropIndexError {
    pub const fn gql_status(&self) -> &'static str {
        match self {
            DropIndexError::NotFound(_) => "42N51",
            DropIndexError::Unsupported(_) => "0A000",
        }
    }
}

/// Renders an index schema as `(:Label {prop1, prop2})` for diagnostics.
/// Public so the `#[error(...)]` `thiserror` format strings can call it.
pub fn format_index_schema(def: &IndexDefinition) -> String {
    let label_part = def
        .label
        .as_deref()
        .map(|l| format!(":{l}"))
        .unwrap_or_else(|| "*".to_string());
    if def.properties.is_empty() {
        format!("({label_part})")
    } else {
        format!("({label_part} {{{}}})", def.properties.join(", "))
    }
}

impl IndexCatalog {
    pub fn list(&self) -> Vec<IndexDefinition> {
        self.by_name.values().cloned().collect()
    }

    pub fn get(&self, name: &str) -> Option<&IndexDefinition> {
        self.by_name.get(name)
    }

    pub fn contains_name(&self, name: &str) -> bool {
        self.by_name.contains_key(name)
    }

    /// Look up an existing entry whose `(kind, entity, label, properties)`
    /// match the supplied request. The token-lookup variant collapses to
    /// `(kind, entity)` because there is at most one token index per entity.
    pub fn find_equivalent(&self, request: &IndexRequest) -> Option<&IndexDefinition> {
        self.by_name.values().find(|def| equivalent(def, request))
    }

    /// Generate a deterministic auto name when the user did not supply one.
    /// Names are stable for a given (kind, entity, label, properties) tuple
    /// within a session.
    pub fn next_auto_name(&mut self, request: &IndexRequest) -> String {
        let base = match &request.label {
            Some(label) => format!(
                "index_{}_{}_{}_{}",
                request.kind.as_str().to_lowercase(),
                request.entity.as_str().to_lowercase(),
                label,
                request.properties.join("_")
            ),
            None => format!(
                "index_{}_{}",
                request.kind.as_str().to_lowercase(),
                request.entity.as_str().to_lowercase()
            ),
        };
        let mut name = base.clone();
        while self.by_name.contains_key(&name) {
            self.auto_seq += 1;
            name = format!("{base}_{}", self.auto_seq);
        }
        name
    }

    pub fn try_create(
        &mut self,
        request: IndexRequest,
        if_not_exists: bool,
    ) -> Result<CreateIndexOutcome, CreateIndexError> {
        // 1) name conflict trumps schema conflict (matches Neo4j precedence).
        let provided_name = request.explicit_name.clone();
        if let Some(name) = provided_name.as_ref() {
            if let Some(existing) = self.by_name.get(name) {
                let existing_clone = existing.clone();
                if if_not_exists {
                    return Ok(CreateIndexOutcome::NoOpExists(existing_clone));
                }
                return Err(CreateIndexError::DuplicateName(existing_clone));
            }
        }

        // 2) schema-level equivalence check (any kind matching the same shape).
        if let Some(existing) = self.find_equivalent(&request) {
            let existing_clone = existing.clone();
            if if_not_exists {
                return Ok(CreateIndexOutcome::NoOpExists(existing_clone));
            }
            return Err(CreateIndexError::EquivalentIndexExists(existing_clone));
        }

        let name = match provided_name {
            Some(name) => name,
            None => self.next_auto_name(&request),
        };

        let def = IndexDefinition {
            name: name.clone(),
            kind: request.kind,
            entity: request.entity,
            label: request.label,
            properties: request.properties,
            options: request.options,
            state: StoredIndexState::Online,
        };
        self.by_name.insert(name, def.clone());
        Ok(CreateIndexOutcome::Created(def))
    }

    pub fn try_drop(
        &mut self,
        name: &str,
        if_exists: bool,
    ) -> Result<DropIndexOutcome, DropIndexError> {
        match self.by_name.remove(name) {
            Some(def) => Ok(DropIndexOutcome::Dropped(def)),
            None if if_exists => Ok(DropIndexOutcome::NoOpMissing),
            None => Err(DropIndexError::NotFound(name.to_string())),
        }
    }
}

fn equivalent(def: &IndexDefinition, request: &IndexRequest) -> bool {
    if def.kind != request.kind || def.entity != request.entity {
        return false;
    }
    match request.kind {
        StoredIndexKind::Lookup => true, // one per (kind, entity)
        _ => def.label == request.label && def.properties == request.properties,
    }
}

/// What a caller asks the catalog to create. The `explicit_name`
/// is `None` when the user did not name the index at the call site —
/// the catalog will mint a deterministic one.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct IndexRequest {
    pub explicit_name: Option<String>,
    pub kind: StoredIndexKind,
    pub entity: StoredIndexEntity,
    pub label: Option<String>,
    pub properties: Vec<String>,
    pub options: BTreeMap<String, IndexConfigValue>,
}
