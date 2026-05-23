//! Graph-shaped value types: identifiers, properties, and the
//! `NodeRecord` / `RelationshipRecord` envelopes every backend stores.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::sync::Arc;

use lora_ast::Direction;

use super::PropertyValue;

pub type NodeId = u64;
pub type RelationshipId = u64;

/// A node's or relationship's property bag.
///
/// Keys are `Arc<str>` rather than `String` so that every node sharing
/// a property name reuses one byte buffer instead of duplicating it.
/// On a 5M-row × 8-column import that turns ~40M heap copies of the
/// column names into ~8 (one per distinct key) plus refcount bumps.
/// Lookups still accept `&str` because `Arc<str>: Borrow<str>`, so
/// most reader code paths are untouched.
///
/// Construction at hot paths should route keys through
/// [`crate::intern`] so the `Arc<str>` instances actually share their
/// backing storage; calling `Arc::from(s)` directly works but
/// allocates a fresh buffer per call.
pub type Properties = BTreeMap<Arc<str>, PropertyValue>;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NodeRecord {
    pub id: NodeId,
    pub labels: Vec<String>,
    pub properties: Properties,
}

impl NodeRecord {
    pub fn has_label(&self, label: &str) -> bool {
        self.labels.iter().any(|l| l == label)
    }

    pub fn property(&self, key: &str) -> Option<&PropertyValue> {
        self.properties.get(key)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RelationshipRecord {
    pub id: RelationshipId,
    pub src: NodeId,
    pub dst: NodeId,
    pub rel_type: String,
    pub properties: Properties,
}

impl RelationshipRecord {
    pub fn property(&self, key: &str) -> Option<&PropertyValue> {
        self.properties.get(key)
    }

    pub fn other_node(&self, node_id: NodeId) -> Option<NodeId> {
        if self.src == node_id {
            Some(self.dst)
        } else if self.dst == node_id {
            Some(self.src)
        } else {
            None
        }
    }

    pub fn matches_direction_from(&self, node_id: NodeId, direction: Direction) -> bool {
        match direction {
            Direction::Right => self.src == node_id,
            Direction::Left => self.dst == node_id,
            Direction::Undirected => self.src == node_id || self.dst == node_id,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ExpandedRelationship {
    pub relationship: RelationshipRecord,
    pub other_node: NodeRecord,
}
