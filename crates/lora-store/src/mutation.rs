//! Mutation events and the optional recorder hook.
//!
//! [`MutationEvent`] is the vocabulary a write-ahead log (or any observer —
//! replication, audit, change-data-capture) would append to a durable stream.
//! The enum covers every method on [`GraphStorageMut`]: each event carries
//! exactly the information needed to deterministically re-apply the mutation
//! against an empty store (or a snapshot) and recover the same state.
//!
//! [`MutationRecorder`] is the observer trait. Backends that want to emit
//! events install a recorder via [`InMemoryGraph::set_mutation_recorder`].
//! The default is `None` so zero-WAL workloads pay only a null-pointer check
//! per mutation — no allocation, no clone.
//!
//! This module does not include any persistent WAL implementation. That
//! would live in a future `lora-wal` crate (or similar) and implement
//! `MutationRecorder` by appending each event to an on-disk log. Snapshot
//! headers already reserve a `wal_lsn` field so a checkpoint = snapshot +
//! log-truncation remains trivially expressible.

use serde::{Deserialize, Serialize};

use crate::{NodeId, Properties, PropertyValue, RelationshipId};

/// A durable, replayable mutation against a graph store.
///
/// Each variant mirrors a method on `GraphStorageMut`. Applying every event
/// in order against a store initialised from the snapshot whose `wal_lsn`
/// immediately precedes the first event reproduces the committed state.
///
/// The enum derives `Serialize`/`Deserialize` so a WAL implementation can
/// bincode-append events straight to disk without a second serialization
/// layer.
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
/// from any thread holding the store's mutex.
pub trait MutationRecorder: Send + Sync + 'static {
    fn record(&self, event: &MutationEvent);
}

/// Convenience adapter that turns any `Fn(&MutationEvent) + Send + Sync`
/// into a `MutationRecorder` — useful in tests and for quick wiring.
pub struct ClosureRecorder<F>(pub F)
where
    F: Fn(&MutationEvent) + Send + Sync + 'static;

impl<F> MutationRecorder for ClosureRecorder<F>
where
    F: Fn(&MutationEvent) + Send + Sync + 'static,
{
    fn record(&self, event: &MutationEvent) {
        (self.0)(event)
    }
}
