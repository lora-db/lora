//! Mutation events and the optional recorder hook.
//!
//! [`MutationEvent`] is the vocabulary a write-ahead log (or any observer —
//! replication, audit, change-data-capture) appends to a durable stream.
//! The enum covers every method on [`GraphStorageMut`]: each event carries
//! exactly the information needed to deterministically re-apply the mutation
//! against an empty store (or a snapshot) and recover the same state.
//!
//! [`MutationRecorder`] is the observer trait. Backends that want to emit
//! events install a recorder via [`InMemoryGraph::set_mutation_recorder`].
//! The default is `None` so zero-WAL workloads pay only a null-pointer check
//! per mutation — no allocation, no clone.
//!
//! The persistent WAL implementation lives in the `lora-wal` crate, which
//! supplies a `WalRecorder` that implements `MutationRecorder` by
//! appending each event to an on-disk log. The snapshot header's
//! `wal_lsn` field is what makes the checkpoint hybrid expressible
//! across crate boundaries without `lora-store` learning about the WAL.

use serde::{Deserialize, Serialize};

use crate::{NodeId, Properties, PropertyValue, RelationshipId};

/// A durable, replayable mutation against a graph store.
///
/// Each variant mirrors a method on `GraphStorageMut`. Applying every event
/// in order against a store initialised from the snapshot whose `wal_lsn`
/// immediately precedes the first event reproduces the committed state.
///
/// The enum derives `Serialize`/`Deserialize` for non-WAL observers and
/// tooling; the production WAL uses its own compact tagged codec.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum MutationEvent {
    CreateNode {
        /// Id the backend allocated for the new node. Captured so replay
        /// against a clean store produces the same id assignment as the
        /// original (`next_node_id` advances deterministically).
        id: NodeId,
        labels: Vec<String>,
        properties: Properties,
    },
    CreateRelationship {
        id: RelationshipId,
        src: NodeId,
        dst: NodeId,
        rel_type: String,
        properties: Properties,
    },
    SetNodeProperty {
        node_id: NodeId,
        key: String,
        value: PropertyValue,
    },
    RemoveNodeProperty {
        node_id: NodeId,
        key: String,
    },
    AddNodeLabel {
        node_id: NodeId,
        label: String,
    },
    RemoveNodeLabel {
        node_id: NodeId,
        label: String,
    },
    SetRelationshipProperty {
        rel_id: RelationshipId,
        key: String,
        value: PropertyValue,
    },
    RemoveRelationshipProperty {
        rel_id: RelationshipId,
        key: String,
    },
    DeleteRelationship {
        rel_id: RelationshipId,
    },
    DeleteNode {
        node_id: NodeId,
    },
    DetachDeleteNode {
        node_id: NodeId,
    },
    Clear,
}

/// Observer that receives every successful mutation in the order the store
/// applied it.
///
/// The recorder sees events *after* the mutation has been applied to the
/// in-memory state, so it never observes a mutation that the store
/// rejected (invalid id, empty relationship type, …). This matches the
/// classic write-ahead-log convention of logging committed changes only.
///
/// Implementations must be `Send + Sync` so a shared recorder can be driven
/// from any thread holding the store's write lock.
pub trait MutationRecorder: Send + Sync + 'static {
    fn record(&self, event: MutationEvent);

    /// Sticky failure flag for durability-shaped recorders.
    ///
    /// `record` itself is infallible — non-WAL observers (audit taps,
    /// replication shadows, CDC sinks) should not abort a write because
    /// their downstream queue is full. Recorders that *do* care about
    /// durability — most importantly the WAL adapter — flip a flag when
    /// an append fails and surface it here. The host (typically
    /// `Database::execute_with_params`) polls this once per critical
    /// section while still holding the store write lock; if poisoned, the
    /// query fails loudly and the caller observes the durability error
    /// rather than a silently-lost write.
    ///
    /// The default returns `None`, so existing recorders compile
    /// unchanged.
    fn poisoned(&self) -> Option<String> {
        None
    }
}

/// Convenience adapter that turns any `Fn(MutationEvent) + Send + Sync`
/// into a `MutationRecorder` — useful in tests and for quick wiring.
pub struct ClosureRecorder<F>(pub F)
where
    F: Fn(MutationEvent) + Send + Sync + 'static;

impl<F> MutationRecorder for ClosureRecorder<F>
where
    F: Fn(MutationEvent) + Send + Sync + 'static,
{
    fn record(&self, event: MutationEvent) {
        (self.0)(event)
    }
}

/// Set of record ids touched by a buffered [`MutationEvent`] stream.
///
/// Built incrementally as events buffer (or in one pass at commit
/// time) by [`MutationWriteSet::extend_from_events`]. Used by the OCC
/// auto-commit path to (a) sort lock-acquire on commit, (b) validate
/// per-record Arc identity against the snapshot.
#[derive(Debug, Default, Clone)]
pub struct MutationWriteSet {
    /// Nodes whose record was created, modified, or deleted.
    pub nodes: std::collections::BTreeSet<NodeId>,
    /// Relationships whose record was created, modified, or deleted.
    pub rels: std::collections::BTreeSet<RelationshipId>,
    /// `true` if the stream contained a `MutationEvent::Clear`. A
    /// clear invalidates any per-record check — the writer must
    /// fall back to a full-graph commit (or fail under OCC).
    pub cleared: bool,
}

impl MutationWriteSet {
    pub fn new() -> Self {
        Self::default()
    }

    /// Walk a `MutationEvent` stream and accumulate every touched
    /// record id. Variants that touch two records (e.g.
    /// `CreateRelationship` mentions both `src` and `dst` plus the
    /// new relationship) record both nodes — the writer's view of
    /// those nodes' adjacency changed too.
    pub fn extend_from_events<'a>(&mut self, events: impl IntoIterator<Item = &'a MutationEvent>) {
        for event in events {
            match event {
                MutationEvent::CreateNode { id, .. } => {
                    self.nodes.insert(*id);
                }
                MutationEvent::CreateRelationship { id, src, dst, .. } => {
                    self.rels.insert(*id);
                    self.nodes.insert(*src);
                    self.nodes.insert(*dst);
                }
                MutationEvent::SetNodeProperty { node_id, .. }
                | MutationEvent::RemoveNodeProperty { node_id, .. }
                | MutationEvent::AddNodeLabel { node_id, .. }
                | MutationEvent::RemoveNodeLabel { node_id, .. } => {
                    self.nodes.insert(*node_id);
                }
                MutationEvent::SetRelationshipProperty { rel_id, .. }
                | MutationEvent::RemoveRelationshipProperty { rel_id, .. } => {
                    self.rels.insert(*rel_id);
                }
                MutationEvent::DeleteRelationship { rel_id } => {
                    self.rels.insert(*rel_id);
                }
                MutationEvent::DeleteNode { node_id } => {
                    self.nodes.insert(*node_id);
                }
                MutationEvent::DetachDeleteNode { node_id } => {
                    // Detach-delete also touches every incident
                    // relationship, but those fire as
                    // `DeleteRelationship` events of their own and
                    // the surrounding loop will pick them up.
                    self.nodes.insert(*node_id);
                }
                MutationEvent::Clear => {
                    self.cleared = true;
                }
            }
        }
    }

    pub fn is_empty(&self) -> bool {
        !self.cleared && self.nodes.is_empty() && self.rels.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn write_set_extracts_ids_from_events() {
        let events = [
            MutationEvent::CreateNode {
                id: 1,
                labels: vec!["A".into()],
                properties: Default::default(),
            },
            MutationEvent::CreateRelationship {
                id: 10,
                src: 1,
                dst: 2,
                rel_type: "R".into(),
                properties: Default::default(),
            },
            MutationEvent::SetNodeProperty {
                node_id: 3,
                key: "x".into(),
                value: PropertyValue::Int(5),
            },
            MutationEvent::DeleteRelationship { rel_id: 11 },
        ];

        let mut ws = MutationWriteSet::new();
        ws.extend_from_events(events.iter());

        // CreateRelationship pulls in src=1, dst=2 alongside its own rel id.
        assert_eq!(ws.nodes.iter().copied().collect::<Vec<_>>(), vec![1, 2, 3]);
        assert_eq!(ws.rels.iter().copied().collect::<Vec<_>>(), vec![10, 11]);
        assert!(!ws.cleared);
    }

    #[test]
    fn write_set_clear_event_is_sticky() {
        let mut ws = MutationWriteSet::new();
        ws.extend_from_events([&MutationEvent::Clear]);
        assert!(ws.cleared);
        assert!(!ws.is_empty()); // cleared counts as non-empty
    }
}
