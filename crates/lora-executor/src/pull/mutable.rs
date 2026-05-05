use std::collections::{BTreeMap, BTreeSet};
use std::mem::ManuallyDrop;
use std::sync::Arc;

use lora_compiler::physical::{PhysicalNodeId, PhysicalPlan};
use lora_compiler::CompiledQuery;
use lora_store::{GraphStorage, GraphStorageMut};

use crate::errors::{ExecResult, ExecutorError};
use crate::executor::{GroupValueKey, MutableExecutionContext, MutableExecutor};
use crate::value::{LoraValue, Row};

use super::traits::write_op_input;
use super::{build_streaming, subtree_is_fully_streaming, BufferedRowSource, RowSource};

/// Pull-based read-write executor. Wraps the existing
/// [`MutableExecutor`] under the same row-cursor API. Mutations are
/// applied during `open_compiled`; the returned cursor yields the
/// resulting rows lazily.
pub struct MutablePullExecutor<'a, S: GraphStorageMut> {
    storage: &'a mut S,
    params: BTreeMap<String, LoraValue>,
}

impl<'a, S: GraphStorageMut + GraphStorage> MutablePullExecutor<'a, S> {
    pub fn new(storage: &'a mut S, params: BTreeMap<String, LoraValue>) -> Self {
        Self { storage, params }
    }

    /// Open a cursor for a compiled write query.
    ///
    /// Fast path: when a branch root is one of `Create` / `Set` /
    /// `Delete` / `Remove` / `Merge` and its input subtree is fully
    /// streamable, returns a [`StreamingWriteCursor`] that pulls input
    /// row-by-row and applies the per-row write through
    /// [`MutableExecutor::apply_write_op`]. `UNION ALL` plans stream
    /// one branch at a time. Plain `UNION` drains branches first so
    /// rows can be deduplicated by name.
    ///
    /// Fallback: a branch that is not streamable materializes through
    /// [`MutableExecutor::execute_rows`] and wraps the result in a
    /// [`BufferedRowSource`].
    pub fn open_compiled(self, compiled: &'a CompiledQuery) -> ExecResult<Box<dyn RowSource + 'a>>
    where
        S: 'a,
    {
        if compiled.unions.is_empty() {
            return open_mutable_plan_cursor(self.storage, &compiled.physical, self.params);
        }

        MutableUnionSource::open(self.storage, compiled, self.params)
            .map(|source| Box::new(source) as Box<dyn RowSource + 'a>)
    }
}

fn open_mutable_plan_cursor<'a, S: GraphStorageMut + GraphStorage + 'a>(
    storage: &'a mut S,
    plan: &'a PhysicalPlan,
    params: BTreeMap<String, LoraValue>,
) -> ExecResult<Box<dyn RowSource + 'a>> {
    if let Some(input) = write_op_input(plan, plan.root) {
        if subtree_is_fully_streaming(plan, input) {
            return StreamingWriteCursor::open(storage, plan, plan.root, params)
                .map(|c| Box::new(c) as Box<dyn RowSource + 'a>);
        }
    }

    let mut executor = MutableExecutor::new(MutableExecutionContext { storage, params });
    let rows = executor.execute_rows(plan)?;
    Ok(Box::new(BufferedRowSource::new(rows)))
}

#[derive(Clone, Copy)]
struct StoragePtr<S> {
    ptr: *mut S,
}

impl<S> StoragePtr<S> {
    fn from_mut(storage: &mut S) -> Self {
        Self {
            ptr: storage as *mut S,
        }
    }

    unsafe fn as_ref<'a>(&self) -> &'a S {
        unsafe { &*self.ptr }
    }

    unsafe fn as_mut<'a>(&self) -> &'a mut S {
        unsafe { &mut *self.ptr }
    }
}

/// Mutable UNION cursor. `UNION ALL` streams one branch at a time
/// against the same staged graph. Plain `UNION` streams branch-by-branch
/// while retaining only a seen-key set for deduplication.
pub struct MutableUnionSource<'a, S: GraphStorageMut + GraphStorage + 'a> {
    storage_ptr: StoragePtr<S>,
    compiled: &'a CompiledQuery,
    params: BTreeMap<String, LoraValue>,
    branch_idx: usize,
    current: Option<Box<dyn RowSource + 'a>>,
    needs_dedup: bool,
    seen: BTreeSet<Vec<(String, GroupValueKey)>>,
    _phantom: std::marker::PhantomData<&'a mut S>,
}

impl<'a, S: GraphStorageMut + GraphStorage + 'a> MutableUnionSource<'a, S> {
    fn open(
        storage: &'a mut S,
        compiled: &'a CompiledQuery,
        params: BTreeMap<String, LoraValue>,
    ) -> ExecResult<Self> {
        let needs_dedup = compiled.unions.iter().any(|branch| !branch.all);
        Ok(Self {
            storage_ptr: StoragePtr::from_mut(storage),
            compiled,
            params,
            branch_idx: 0,
            current: None,
            needs_dedup,
            seen: BTreeSet::new(),
            _phantom: std::marker::PhantomData,
        })
    }

    fn branch_count(&self) -> usize {
        self.compiled.unions.len() + 1
    }

    fn branch_plan(&self, idx: usize) -> &'a PhysicalPlan {
        if idx == 0 {
            &self.compiled.physical
        } else {
            &self.compiled.unions[idx - 1].physical
        }
    }

    fn open_branch(&mut self, idx: usize) -> ExecResult<Box<dyn RowSource + 'a>> {
        let plan = self.branch_plan(idx);
        // SAFETY: MutableUnionSource keeps at most one branch cursor
        // alive at a time. `current` is dropped before advancing to
        // the next branch, so each mutable reborrow is temporally
        // disjoint.
        let storage = unsafe { self.storage_ptr.as_mut() };
        open_mutable_plan_cursor(storage, plan, self.params.clone())
    }
}

impl<'a, S: GraphStorageMut + GraphStorage + 'a> RowSource for MutableUnionSource<'a, S> {
    fn next_row(&mut self) -> ExecResult<Option<Row>> {
        loop {
            if self.branch_idx >= self.branch_count() {
                return Ok(None);
            }

            if self.current.is_none() {
                self.current = Some(self.open_branch(self.branch_idx)?);
            }

            match self
                .current
                .as_mut()
                .expect("current branch initialized above")
                .next_row()?
            {
                Some(row) => {
                    if self.needs_dedup {
                        let key = row
                            .iter_named()
                            .map(|(_, name, val)| {
                                (name.into_owned(), GroupValueKey::from_value(val))
                            })
                            .collect();
                        if !self.seen.insert(key) {
                            continue;
                        }
                    }
                    return Ok(Some(row));
                }
                None => {
                    self.current.take();
                    self.branch_idx += 1;
                }
            }
        }
    }
}

/// Streaming write cursor for plans whose root is one of
/// `Create` / `Set` / `Delete` / `Remove` / `Merge` and whose input
/// subtree is fully streamable.
///
/// # Layout invariant
///
/// The cursor owns a raw alias of the original `&'a mut S`.
/// Its `upstream` was constructed using a `&'a S` reborrow derived
/// from `storage_ptr` via unsafe lifetime extension. This is sound
/// because the existing read-side `RowSource` impls (see
/// `NodeScanSource::cur_ids`, `ExpandSource::cur_edges`, etc.)
/// materialize their iteration state into owned `Vec`s at
/// construction or first call, so no live `&S` borrow into storage
/// persists across `next_row` calls. Read-only access happens
/// transiently inside each `upstream.next_row` call; mutable access
/// happens between calls inside [`MutableExecutor::apply_write_op`].
/// The borrows never overlap in time.
///
/// # Drop order
///
/// `upstream` must drop before any caller may regain `&mut S` access
/// to the underlying storage. The explicit `Drop` impl enforces
/// that order — `ManuallyDrop` lets us force the sequence.
pub struct StreamingWriteCursor<'a, S: GraphStorageMut + GraphStorage + 'a> {
    /// SAFETY: borrows from `*storage_ptr`. Must drop first.
    upstream: ManuallyDrop<Box<dyn RowSource + 'a>>,
    /// Raw alias of the `&'a mut S` handed in at construction. Used
    /// as `&S` by `upstream` and as `&mut S` inside this cursor's `next_row`.
    storage_ptr: StoragePtr<S>,
    /// Physical plan — kept alive for the per-row op borrow.
    plan: &'a PhysicalPlan,
    /// Index into `plan.nodes` of the write operator.
    /// We re-fetch the op per call so this struct doesn't need to
    /// be parameterized by the specific op type.
    write_op_node: PhysicalNodeId,
    /// Parameters; cloned per row into a fresh `MutableExecutor`.
    /// In typical bulk-write workloads this is empty or tiny.
    params: BTreeMap<String, LoraValue>,
    _phantom: std::marker::PhantomData<&'a mut S>,
}

impl<'a, S: GraphStorageMut + GraphStorage + 'a> StreamingWriteCursor<'a, S> {
    /// Build a cursor. Caller must already have verified that
    /// `plan.nodes[write_op_node]` is a streamable write op via
    /// [`write_op_input`] and [`subtree_is_fully_streaming`].
    pub(crate) fn open(
        storage: &'a mut S,
        plan: &'a PhysicalPlan,
        write_op_node: PhysicalNodeId,
        params: BTreeMap<String, LoraValue>,
    ) -> ExecResult<Self> {
        let input = match write_op_input(plan, write_op_node) {
            Some(i) => i,
            None => {
                return Err(ExecutorError::RuntimeError(format!(
                    "StreamingWriteCursor::open called with non-write node {write_op_node:?}"
                )));
            }
        };
        let storage_ptr = StoragePtr::from_mut(storage);

        // SAFETY: see struct-level comment.
        let storage_ref: &'a S = unsafe { storage_ptr.as_ref() };
        let upstream = build_streaming(plan, input, storage_ref, Arc::new(params.clone()))?;

        Ok(Self {
            upstream: ManuallyDrop::new(upstream),
            storage_ptr,
            plan,
            write_op_node,
            params,
            _phantom: std::marker::PhantomData,
        })
    }
}

impl<'a, S: GraphStorageMut + GraphStorage + 'a> RowSource for StreamingWriteCursor<'a, S> {
    fn next_row(&mut self) -> ExecResult<Option<Row>> {
        let mut row = match self.upstream.next_row()? {
            Some(r) => r,
            None => return Ok(None),
        };

        // SAFETY: upstream's `next_row` has returned, so its
        // dormant `&S` borrow is not in active use right now. We
        // reborrow `&mut S` for the per-row write and drop the
        // borrow before the next pull.
        let storage_mut: &mut S = unsafe { self.storage_ptr.as_mut() };
        let mut exec = MutableExecutor::new(MutableExecutionContext {
            storage: storage_mut,
            params: self.params.clone(),
        });
        let op = &self.plan.nodes[self.write_op_node];
        exec.apply_write_op(op, &mut row)?;
        let row = exec.hydrate_row(row);
        Ok(Some(row))
    }
}

impl<'a, S: GraphStorageMut + GraphStorage + 'a> Drop for StreamingWriteCursor<'a, S> {
    fn drop(&mut self) {
        // SAFETY: drop `upstream` first to release its borrow into
        // `*storage_ptr`. Subsequent fields drop via the normal
        // field-drop sequence and don't touch storage.
        unsafe {
            ManuallyDrop::drop(&mut self.upstream);
        }
    }
}
