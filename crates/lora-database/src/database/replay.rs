//! Mutation-event replay helpers used by recovery and any caller that
//! needs to apply a stream of [`MutationEvent`]s against a graph.
//!
//! Two concerns live here:
//!
//! * [`install_recorder_if_inmemory`] — best-effort attach/detach of a
//!   [`MutationRecorder`] when the storage happens to be `InMemoryGraph`.
//! * [`replay_into`] — the WAL-recovery replay entry point: applies a
//!   committed event stream to a fresh graph using id-preserving
//!   create paths, with descriptive errors keyed by event index.

use std::any::Any;
use std::sync::Arc;

use anyhow::{anyhow, Result};

use lora_store::{GraphStorage, GraphStorageMut, InMemoryGraph, MutationEvent, MutationRecorder};

/// Best-effort: install a mutation recorder on the storage when the
/// concrete type is `InMemoryGraph`. The WAL's recorder lives on the
/// store, but `GraphStorage` is an open trait — backends that don't
/// support recorders are simply skipped (their writes go unobserved
/// for WAL purposes, which is correct because they don't drive the WAL
/// in the first place).
pub(crate) fn install_recorder_if_inmemory<S: GraphStorage + Any + Sized>(
    store: &mut S,
    recorder: Option<Arc<dyn MutationRecorder>>,
) {
    let any: &mut dyn Any = store;
    if let Some(graph) = any.downcast_mut::<InMemoryGraph>() {
        graph.set_mutation_recorder(recorder);
    }
}

/// Apply a `MutationEvent` stream to an in-memory graph by dispatching
/// each variant to the matching store operation.
///
/// Creation events are replayed through id-preserving paths, not the
/// normal allocator-backed mutation methods. That matters after aborted
/// transactions: an aborted create can consume id `N` in the original
/// process, be dropped by replay, and leave the next committed create at
/// id `N + 1`. Reusing the regular allocator would shift ids downward.
///
/// Replay must be invoked **before** the `WalRecorder` is installed
/// on the graph. Otherwise the replay's own mutations would fire the
/// recorder and re-write the same events to the WAL, doubling them on
/// the next recovery.
pub(crate) fn replay_into(graph: &mut InMemoryGraph, events: Vec<MutationEvent>) -> Result<()> {
    for (idx, event) in events.into_iter().enumerate() {
        match event {
            MutationEvent::CreateNode {
                id,
                labels,
                properties,
            } => {
                graph
                    .replay_create_node(id, labels, properties)
                    .map_err(|e| anyhow!("WAL replay failed at event {idx}: {e}"))?;
            }
            MutationEvent::CreateRelationship {
                id,
                src,
                dst,
                rel_type,
                properties,
            } => {
                graph
                    .replay_create_relationship(id, src, dst, &rel_type, properties)
                    .map_err(|e| anyhow!("WAL replay failed at event {idx}: {e}"))?;
            }
            MutationEvent::SetNodeProperty {
                node_id,
                key,
                value,
            } => {
                if !graph.set_node_property(node_id, key, value) {
                    return Err(anyhow!(
                        "WAL replay failed at event {idx}: missing node {node_id} for property set"
                    ));
                }
            }
            MutationEvent::RemoveNodeProperty { node_id, key } => {
                if !graph.remove_node_property(node_id, &key) {
                    return Err(anyhow!(
                        "WAL replay failed at event {idx}: missing node {node_id} for property removal"
                    ));
                }
            }
            MutationEvent::AddNodeLabel { node_id, label } => {
                if !graph.add_node_label(node_id, &label) {
                    return Err(anyhow!(
                        "WAL replay failed at event {idx}: missing node {node_id} for label add"
                    ));
                }
            }
            MutationEvent::RemoveNodeLabel { node_id, label } => {
                if !graph.remove_node_label(node_id, &label) {
                    return Err(anyhow!(
                        "WAL replay failed at event {idx}: missing node {node_id} for label removal"
                    ));
                }
            }
            MutationEvent::SetRelationshipProperty { rel_id, key, value } => {
                if !graph.set_relationship_property(rel_id, key, value) {
                    return Err(anyhow!(
                        "WAL replay failed at event {idx}: missing relationship {rel_id} for property set"
                    ));
                }
            }
            MutationEvent::RemoveRelationshipProperty { rel_id, key } => {
                if !graph.remove_relationship_property(rel_id, &key) {
                    return Err(anyhow!(
                        "WAL replay failed at event {idx}: missing relationship {rel_id} for property removal"
                    ));
                }
            }
            MutationEvent::DeleteRelationship { rel_id } => {
                if !graph.delete_relationship(rel_id) {
                    return Err(anyhow!(
                        "WAL replay failed at event {idx}: missing relationship {rel_id} for delete"
                    ));
                }
            }
            MutationEvent::DeleteNode { node_id } => {
                if !graph.delete_node(node_id) {
                    return Err(anyhow!(
                        "WAL replay failed at event {idx}: missing or attached node {node_id} for delete"
                    ));
                }
            }
            MutationEvent::DetachDeleteNode { node_id } => {
                // After the cascading DeleteRelationship +
                // DeleteNode events have already replayed, the node
                // is gone and this becomes a no-op. Calling it
                // anyway is harmless.
                graph.detach_delete_node(node_id);
            }
            MutationEvent::Clear => {
                graph.clear();
            }
        }
    }
    Ok(())
}
