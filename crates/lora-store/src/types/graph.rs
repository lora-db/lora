//! Graph-shaped value types: identifiers, properties, and the
//! `NodeRecord` / `RelationshipRecord` envelopes every backend stores.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

use lora_ast::Direction;

use super::PropertyValue;

pub type NodeId = u64;
pub type RelationshipId = u64;

pub type Properties = BTreeMap<String, PropertyValue>;

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
