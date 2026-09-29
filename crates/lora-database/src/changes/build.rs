//! Turn one committed transaction's [`MutationEvent`]s into [`Change`]s.
//!
//! Events are primitive (`SetNodeProperty`, `AddNodeLabel`, ...), and a
//! transaction may touch the same entity many times. The feed reports the
//! net effect per entity instead: created, updated or deleted, in the order
//! the transaction first touched each entity. Images come from the graph:
//! the post-commit graph for created and updated entities, and the
//! pre-commit graph (captured as [`PreImages`] before the transaction
//! applied) for deleted ones.

use std::collections::hash_map::Entry;
use std::collections::{BTreeMap, HashMap};

use std::sync::Mutex;

use lora_store::{
    BorrowedGraphStorage, DeletedRecordSink, InMemoryGraph, MutationEvent, NodeId, NodeRecord,
    RelationshipId, RelationshipRecord,
};

use super::Change;

/// Records of the entities a transaction deletes, read from the graph as it
/// was before the transaction applied.
#[derive(Default)]
pub(crate) struct PreImages {
    nodes: HashMap<NodeId, NodeRecord>,
    rels: HashMap<RelationshipId, RelationshipRecord>,
}

impl PreImages {
    /// Capture the pre-images `events` will need from `graph`, the state
    /// before the transaction. Only deleted entities that existed before
    /// the transaction are captured.
    pub(crate) fn capture(events: &[MutationEvent], graph: &InMemoryGraph) -> Self {
        let mut out = Self::default();
        for event in events {
            match event {
                MutationEvent::DeleteNode { node_id }
                | MutationEvent::DetachDeleteNode { node_id } => {
                    if let (Entry::Vacant(slot), Some(record)) =
                        (out.nodes.entry(*node_id), graph.node_ref(*node_id))
                    {
                        slot.insert(record.clone());
                    }
                }
                MutationEvent::DeleteRelationship { rel_id } => {
                    if let (Entry::Vacant(slot), Some(record)) =
                        (out.rels.entry(*rel_id), graph.relationship_ref(*rel_id))
                    {
                        slot.insert(record.clone());
                    }
                }
                _ => {}
            }
        }
        out
    }

    /// Pre-images for `events` from the pre-write graph, when there is one
    /// and the events delete something.
    pub(crate) fn for_events(events: &[MutationEvent], pre: Option<&InMemoryGraph>) -> Self {
        match pre {
            Some(pre) if Self::needed_for(events) => Self::capture(events, pre),
            _ => Self::default(),
        }
    }

    /// Whether any event in `events` deletes an entity, so the caller
    /// needs a pre-commit graph to describe it.
    pub(crate) fn needed_for(events: &[MutationEvent]) -> bool {
        events.iter().any(|event| {
            matches!(
                event,
                MutationEvent::DeleteNode { .. }
                    | MutationEvent::DetachDeleteNode { .. }
                    | MutationEvent::DeleteRelationship { .. }
            )
        })
    }
}

/// Collects deleted records while a write mutates the live graph in place
/// (installed as the graph's [`DeletedRecordSink`]).
#[derive(Default)]
pub(crate) struct PreImageSink {
    images: Mutex<PreImages>,
}

impl PreImageSink {
    pub(crate) fn take(&self) -> PreImages {
        std::mem::take(&mut *self.images.lock().unwrap_or_else(|p| p.into_inner()))
    }
}

impl DeletedRecordSink for PreImageSink {
    fn node_deleted(&self, record: &NodeRecord) {
        self.images
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .nodes
            .entry(record.id)
            .or_insert_with(|| record.clone());
    }

    fn relationship_deleted(&self, record: &RelationshipRecord) {
        self.images
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .rels
            .entry(record.id)
            .or_insert_with(|| record.clone());
    }
}

#[derive(Default)]
struct NodeTouch {
    created: bool,
    deleted: bool,
    /// Last operation per property key: `true` = set, `false` = removed.
    keys: BTreeMap<String, bool>,
    /// Last operation per label: `true` = added, `false` = removed.
    labels: BTreeMap<String, bool>,
}

#[derive(Default)]
struct RelTouch {
    created: bool,
    deleted: bool,
    keys: BTreeMap<String, bool>,
}

enum Touched {
    Node(NodeId),
    Rel(RelationshipId),
}

#[derive(Default)]
struct Accumulator {
    reset: bool,
    order: Vec<Touched>,
    nodes: HashMap<NodeId, NodeTouch>,
    rels: HashMap<RelationshipId, RelTouch>,
}

impl Accumulator {
    fn node(&mut self, id: NodeId) -> &mut NodeTouch {
        if !self.nodes.contains_key(&id) {
            self.order.push(Touched::Node(id));
        }
        self.nodes.entry(id).or_default()
    }

    fn rel(&mut self, id: RelationshipId) -> &mut RelTouch {
        if !self.rels.contains_key(&id) {
            self.order.push(Touched::Rel(id));
        }
        self.rels.entry(id).or_default()
    }

    fn observe(&mut self, event: &MutationEvent) {
        match event {
            MutationEvent::CreateNode { id, .. } => self.node(*id).created = true,
            MutationEvent::CreateRelationship { id, .. } => self.rel(*id).created = true,
            MutationEvent::SetNodeProperty { node_id, key, .. } => {
                self.node(*node_id).keys.insert(key.clone(), true);
            }
            MutationEvent::RemoveNodeProperty { node_id, key } => {
                self.node(*node_id).keys.insert(key.clone(), false);
            }
            MutationEvent::AddNodeLabel { node_id, label } => {
                self.node(*node_id).labels.insert(label.clone(), true);
            }
            MutationEvent::RemoveNodeLabel { node_id, label } => {
                self.node(*node_id).labels.insert(label.clone(), false);
            }
            MutationEvent::SetRelationshipProperty { rel_id, key, .. } => {
                self.rel(*rel_id).keys.insert(key.clone(), true);
            }
            MutationEvent::RemoveRelationshipProperty { rel_id, key } => {
                self.rel(*rel_id).keys.insert(key.clone(), false);
            }
            MutationEvent::DeleteRelationship { rel_id } => self.rel(*rel_id).deleted = true,
            MutationEvent::DeleteNode { node_id } | MutationEvent::DetachDeleteNode { node_id } => {
                self.node(*node_id).deleted = true
            }
            MutationEvent::Clear => {
                // Everything before the clear is gone; the batch starts
                // over with a `Reset` followed by whatever came after.
                *self = Accumulator {
                    reset: true,
                    ..Accumulator::default()
                };
            }
            MutationEvent::CreateIndex { .. }
            | MutationEvent::DropIndex { .. }
            | MutationEvent::CreateConstraint { .. }
            | MutationEvent::DropConstraint { .. } => {}
        }
    }
}

fn split_ops(ops: BTreeMap<String, bool>) -> (Vec<String>, Vec<String>) {
    let mut set = Vec::new();
    let mut removed = Vec::new();
    for (key, is_set) in ops {
        if is_set {
            set.push(key);
        } else {
            removed.push(key);
        }
    }
    (set, removed)
}

/// Net changes of one committed transaction. `post` is the graph after the
/// transaction; `pre` holds the records it deleted.
pub(crate) fn build_changes(
    events: &[MutationEvent],
    pre: &PreImages,
    post: &InMemoryGraph,
) -> Vec<Change> {
    let mut acc = Accumulator::default();
    for event in events {
        acc.observe(event);
    }

    let mut out = Vec::with_capacity(acc.order.len() + usize::from(acc.reset));
    if acc.reset {
        out.push(Change::Reset);
    }
    for touched in std::mem::take(&mut acc.order) {
        match touched {
            Touched::Node(id) => {
                let Some(touch) = acc.nodes.remove(&id) else {
                    continue;
                };
                if let Some(change) = node_change(id, touch, pre, post) {
                    out.push(change);
                }
            }
            Touched::Rel(id) => {
                let Some(touch) = acc.rels.remove(&id) else {
                    continue;
                };
                if let Some(change) = rel_change(id, touch, pre, post) {
                    out.push(change);
                }
            }
        }
    }
    out
}

fn node_change(
    id: NodeId,
    touch: NodeTouch,
    pre: &PreImages,
    post: &InMemoryGraph,
) -> Option<Change> {
    match (touch.created, touch.deleted) {
        // Created and deleted inside one transaction: never visible.
        (true, true) => None,
        (_, true) => {
            let (labels, properties) = pre
                .nodes
                .get(&id)
                .map(|record| (record.labels.clone(), record.properties.clone()))
                .unwrap_or_default();
            Some(Change::NodeDeleted {
                id,
                labels,
                properties,
            })
        }
        (true, false) => {
            let record = post.node_ref(id)?;
            Some(Change::NodeCreated {
                id,
                labels: record.labels.clone(),
                properties: record.properties.clone(),
            })
        }
        (false, false) => {
            let record = post.node_ref(id)?;
            let (set_keys, removed_keys) = split_ops(touch.keys);
            let (added_labels, removed_labels) = split_ops(touch.labels);
            Some(Change::NodeUpdated {
                id,
                labels: record.labels.clone(),
                properties: record.properties.clone(),
                set_keys,
                removed_keys,
                added_labels,
                removed_labels,
            })
        }
    }
}

fn rel_change(
    id: RelationshipId,
    touch: RelTouch,
    pre: &PreImages,
    post: &InMemoryGraph,
) -> Option<Change> {
    match (touch.created, touch.deleted) {
        (true, true) => None,
        (_, true) => {
            let record = pre.rels.get(&id)?;
            Some(Change::RelationshipDeleted {
                id,
                rel_type: record.rel_type.clone(),
                start: record.src,
                end: record.dst,
                properties: record.properties.clone(),
            })
        }
        (true, false) => {
            let record = post.relationship_ref(id)?;
            Some(Change::RelationshipCreated {
                id,
                rel_type: record.rel_type.clone(),
                start: record.src,
                end: record.dst,
                properties: record.properties.clone(),
            })
        }
        (false, false) => {
            let record = post.relationship_ref(id)?;
            let (set_keys, removed_keys) = split_ops(touch.keys);
            Some(Change::RelationshipUpdated {
                id,
                rel_type: record.rel_type.clone(),
                start: record.src,
                end: record.dst,
                properties: record.properties.clone(),
                set_keys,
                removed_keys,
            })
        }
    }
}
