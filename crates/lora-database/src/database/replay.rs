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

use lora_store::{
    DeletedRecordSink, GraphStorage, GraphStorageMut, InMemoryGraph, MutationEvent,
    MutationRecorder,
};
use lora_wal::{Lsn, SyncMode, Wal, WalError};

use crate::error::{LoraError, LoraErrorCode};

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

/// Best-effort: install a [`DeletedRecordSink`] when the storage is
/// `InMemoryGraph`.
pub(crate) fn install_deleted_sink_if_inmemory<S: GraphStorage + Any + Sized>(
    store: &mut S,
    sink: Option<Arc<dyn DeletedRecordSink>>,
) {
    let any: &mut dyn Any = store;
    if let Some(graph) = any.downcast_mut::<InMemoryGraph>() {
        graph.set_deleted_record_sink(sink);
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
    replay_from(graph, 0, events)
}

/// Open a WAL, applying each committed transaction to `graph` as replay
/// reads it, so the open holds the graph and one transaction rather than
/// the whole log decoded.
pub(crate) fn open_wal_into(
    graph: &mut InMemoryGraph,
    dir: impl Into<std::path::PathBuf>,
    sync_mode: SyncMode,
    segment_target_bytes: u64,
    checkpoint_lsn: Lsn,
) -> std::result::Result<Arc<Wal>, LoraError> {
    let mut applied = 0usize;
    let mut failure = None;
    let opened = Wal::open_replaying(
        dir,
        sync_mode,
        segment_target_bytes,
        checkpoint_lsn,
        &mut |events| {
            let count = events.len();
            replay_from(graph, applied, events).map_err(|err| {
                let message = err.to_string();
                failure = Some(err);
                WalError::Malformed(message)
            })?;
            applied += count;
            Ok(())
        },
    );
    match (opened, failure) {
        (Ok(wal), _) => Ok(wal),
        (Err(_), Some(err)) => Err(replay_error(err)),
        (Err(err), None) => Err(err.into()),
    }
}

/// A committed WAL transaction that cannot be applied to the graph it
/// follows: the log does not describe a reachable state, so it is reported
/// as WAL corruption with the failing event, not as an internal error.
pub(crate) fn replay_error(err: anyhow::Error) -> LoraError {
    LoraError::new(LoraErrorCode::WalCorruption, format!("{err:#}"))
}

/// [`replay_into`] for events that follow `first_idx` already applied,
/// which numbers the errors.
fn replay_from(
    graph: &mut InMemoryGraph,
    first_idx: usize,
    events: Vec<MutationEvent>,
) -> Result<()> {
    for (idx, event) in (first_idx..).zip(events) {
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
            MutationEvent::CreateIndex {
                request,
                if_not_exists,
            } => {
                graph
                    .replay_create_index(request, if_not_exists)
                    .map_err(|e| anyhow!("WAL replay failed at event {idx}: {e}"))?;
            }
            MutationEvent::DropIndex { name, if_exists } => {
                graph
                    .replay_drop_index(&name, if_exists)
                    .map_err(|e| anyhow!("WAL replay failed at event {idx}: {e}"))?;
            }
            MutationEvent::CreateConstraint {
                request,
                if_not_exists,
            } => {
                graph
                    .replay_create_constraint(request, if_not_exists)
                    .map_err(|e| anyhow!("WAL replay failed at event {idx}: {e}"))?;
            }
            MutationEvent::DropConstraint { name, if_exists } => {
                graph
                    .replay_drop_constraint(&name, if_exists)
                    .map_err(|e| anyhow!("WAL replay failed at event {idx}: {e}"))?;
            }
        }
    }
    Ok(())
}
