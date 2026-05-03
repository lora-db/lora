//! Mutation-event replay and merge helpers shared by recovery, the
//! optimistic-commit path, and any caller that needs to apply a stream
//! of [`MutationEvent`]s against a graph.
//!
//! Three concerns live here:
//!
//! * [`install_recorder_if_inmemory`] — best-effort attach/detach of a
//!   [`MutationRecorder`] when the storage happens to be `InMemoryGraph`.
//! * [`validate_write_set_unchanged`] / [`merge_events_into`] /
//!   [`apply_event`] — Phase 4.2 OCC validation + per-record merge,
//!   replayed onto a fresh clone of the live state so disjoint
//!   concurrent writers preserve each other's updates.
//! * [`replay_into`] — the WAL-recovery replay entry point: applies a
//!   committed event stream to a fresh graph using id-preserving
//!   create paths, with descriptive errors keyed by event index.
//!
//! The OCC and recovery replay paths intentionally differ in their error
//! signal: recovery returns `Err(anyhow!(...))` because a missing entity
//! at recovery time is a corruption, while OCC merge returns `false`
//! because the same condition just means a concurrent writer raced ahead
//! and we should retry from a fresh snapshot.

use std::any::Any;
use std::sync::Arc;

use anyhow::{anyhow, Result};

use lora_store::{
    GraphStorage, GraphStorageMut, InMemoryGraph, MutationEvent, MutationRecorder,
    MutationWriteSet, NodeRecord, RelationshipRecord,
};

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

/// Phase 4.2 per-record validation. Returns true if every record in
/// the write set has the same identity in `current` as it did in
/// `snapshot` — meaning no concurrent writer modified those records
/// between this writer's snapshot and its commit.
///
/// Identity is established by the address of the `&NodeRecord` /
/// `&RelationshipRecord` reached via `with_node` / `with_relationship`:
/// because each record lives behind an `Arc` (Phase 2), two graphs
/// share refcount of the same heap allocation iff their references
/// are pointer-equal. A modified record produces a fresh `Arc::new(...)`
/// at the same id; the pointer differs and the check rejects.
///
/// Returns `false` if the write set contains a `Clear` (we can't
/// merge through a clear), if any record was added/removed between
/// snapshots (slot present-vs-absent mismatch), or if a non-`InMemoryGraph`
/// backend is in use (the helper falls back to coarse Arc::ptr_eq).
pub(crate) fn validate_write_set_unchanged<S: GraphStorage + Any + Sized>(
    snapshot: &S,
    current: &S,
    write_set: &MutationWriteSet,
) -> bool {
    if write_set.cleared {
        return false;
    }

    // Downcast to InMemoryGraph so we can compare per-record Arc
    // identity. Other backends fall back to a coarse "is the whole
    // graph the same Arc" check.
    let snap_any: &dyn Any = snapshot;
    let cur_any: &dyn Any = current;
    let snap_im = snap_any.downcast_ref::<InMemoryGraph>();
    let cur_im = cur_any.downcast_ref::<InMemoryGraph>();
    let (snap, cur) = match (snap_im, cur_im) {
        (Some(s), Some(c)) => (s, c),
        _ => return std::ptr::eq(snapshot as *const S, current as *const S),
    };

    for &id in &write_set.nodes {
        let snap_ptr = snap.with_node(id, |n| n as *const NodeRecord);
        let cur_ptr = cur.with_node(id, |n| n as *const NodeRecord);
        match (snap_ptr, cur_ptr) {
            (Some(a), Some(b)) if std::ptr::eq(a, b) => {}
            (None, None) => {}
            _ => return false,
        }
    }
    for &id in &write_set.rels {
        let snap_ptr = snap.with_relationship(id, |r| r as *const RelationshipRecord);
        let cur_ptr = cur.with_relationship(id, |r| r as *const RelationshipRecord);
        match (snap_ptr, cur_ptr) {
            (Some(a), Some(b)) if std::ptr::eq(a, b) => {}
            (None, None) => {}
            _ => return false,
        }
    }
    true
}

/// Phase 4.2 merge. Apply a writer's buffered `MutationEvent` stream
/// to a fresh clone of the current live state, producing the graph
/// the writer should publish. By replaying onto `current` (rather
/// than the writer's stale snapshot), we preserve any concurrent
/// disjoint writer's updates that landed between this writer's
/// snapshot and its commit.
///
/// Returns `true` on successful merge. `false` indicates the events
/// couldn't be applied (e.g. an id-allocation collision the validation
/// missed) and the caller should retry from a fresh snapshot.
///
/// Only handles `InMemoryGraph`; other backends fall through to a
/// no-op merge (caller publishes the staged copy via the existing
/// Phase 3 path).
pub(crate) fn merge_events_into<S: GraphStorage + Any + Sized>(
    publish_state: &mut S,
    events: &[MutationEvent],
) -> bool {
    let any: &mut dyn Any = publish_state;
    let Some(graph) = any.downcast_mut::<InMemoryGraph>() else {
        // Non-InMemoryGraph backend — caller falls back. Returning
        // true keeps the publish path intact (publishes whatever the
        // caller staged), which is the Phase 3 behavior for those
        // backends.
        return true;
    };

    // The fresh clone has no recorder (clone drops it deliberately),
    // so replay won't double-emit through a downstream WAL.
    for event in events {
        if !apply_event(graph, event) {
            return false;
        }
    }
    true
}

/// Apply a single buffered mutation event to a graph. Mirrors
/// `replay_into` (which is the WAL recovery path) but operates on a
/// borrowed graph so the caller can compose it across an event slice
/// without taking ownership.
pub(crate) fn apply_event(graph: &mut InMemoryGraph, event: &MutationEvent) -> bool {
    match event {
        MutationEvent::CreateNode {
            id,
            labels,
            properties,
        } => graph
            .replay_create_node(*id, labels.clone(), properties.clone())
            .is_ok(),
        MutationEvent::CreateRelationship {
            id,
            src,
            dst,
            rel_type,
            properties,
        } => graph
            .replay_create_relationship(*id, *src, *dst, rel_type, properties.clone())
            .is_ok(),
        MutationEvent::SetNodeProperty {
            node_id,
            key,
            value,
        } => graph.set_node_property(*node_id, key.clone(), value.clone()),
        MutationEvent::RemoveNodeProperty { node_id, key } => {
            graph.remove_node_property(*node_id, key)
        }
        MutationEvent::AddNodeLabel { node_id, label } => graph.add_node_label(*node_id, label),
        MutationEvent::RemoveNodeLabel { node_id, label } => {
            graph.remove_node_label(*node_id, label)
        }
        MutationEvent::SetRelationshipProperty { rel_id, key, value } => {
            graph.set_relationship_property(*rel_id, key.clone(), value.clone())
        }
        MutationEvent::RemoveRelationshipProperty { rel_id, key } => {
            graph.remove_relationship_property(*rel_id, key)
        }
        MutationEvent::DeleteRelationship { rel_id } => graph.delete_relationship(*rel_id),
        MutationEvent::DeleteNode { node_id } => graph.delete_node(*node_id),
        MutationEvent::DetachDeleteNode { node_id } => graph.detach_delete_node(*node_id),
        MutationEvent::Clear => {
            graph.clear();
            true
        }
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
