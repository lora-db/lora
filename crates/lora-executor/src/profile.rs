//! Profile-mode instrumentation for the pull executor.
//!
//! When a `MetricsCollector` is installed via [`install_collector`], the
//! pull builder wraps each operator's [`RowSource`] in a [`MeteredSource`]
//! that records, for that operator id:
//!  - `next_calls`: number of `next_row` invocations.
//!  - `rows`: number of `Some(_)` rows produced.
//!  - `elapsed_ns`: cumulative wall-clock time spent inside `next_row`,
//!    *inclusive* of time spent pulling from upstream operators. This
//!    matches the "operator + descendants" view that is most actionable
//!    when reading a profile.
//!
//! The collector is installed for the lifetime of one query through a
//! thread-local guard; the executor is single-threaded per-query, so this
//! is safe and avoids threading an extra parameter through every source
//! constructor. Outside `profile()` calls the thread-local is `None` and
//! `wrap_metered` returns its argument unchanged — `execute()` and
//! streaming reads pay nothing.

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use web_time::Instant;

use lora_compiler::physical::PhysicalNodeId;

use crate::errors::ExecResult;
use crate::pull::RowSource;
use crate::value::Row;

/// Per-operator metrics collected during a profile run.
#[derive(Debug, Clone, Default)]
pub struct OperatorProfile {
    pub rows: u64,
    pub elapsed_ns: u64,
    pub next_calls: u64,
}

/// Shared accumulator. Pass an `Arc` clone to each `MeteredSource` so
/// every operator writes into the same map.
#[derive(Debug, Default)]
pub struct MetricsCollector {
    inner: Mutex<BTreeMap<PhysicalNodeId, OperatorProfile>>,
}

impl MetricsCollector {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn record(&self, op: PhysicalNodeId, elapsed_ns: u64, produced_row: bool) {
        if let Ok(mut map) = self.inner.lock() {
            let entry = map.entry(op).or_default();
            entry.next_calls += 1;
            entry.elapsed_ns = entry.elapsed_ns.saturating_add(elapsed_ns);
            if produced_row {
                entry.rows += 1;
            }
        }
    }

    pub fn snapshot(&self) -> BTreeMap<PhysicalNodeId, OperatorProfile> {
        self.inner.lock().map(|m| m.clone()).unwrap_or_default()
    }
}

thread_local! {
    static CURRENT: RefCell<Option<Arc<MetricsCollector>>> = const { RefCell::new(None) };
}

/// RAII guard. While alive, the thread-local current collector is set
/// to the supplied collector; on drop the one installed before it (if any)
/// is restored, so a profiled query nested in another keeps the outer
/// one's metering.
pub struct CollectorGuard {
    prev: Option<Arc<MetricsCollector>>,
}

impl CollectorGuard {
    pub fn install(collector: Arc<MetricsCollector>) -> Self {
        let prev = CURRENT.with(|cell| cell.borrow_mut().replace(collector));
        Self { prev }
    }
}

impl Drop for CollectorGuard {
    fn drop(&mut self) {
        let prev = self.prev.take();
        CURRENT.with(|cell| {
            *cell.borrow_mut() = prev;
        });
    }
}

/// Wrap `inner` with timing instrumentation if a collector is currently
/// installed; otherwise return `inner` unchanged.
pub(crate) fn wrap_metered<'a>(
    op_id: PhysicalNodeId,
    inner: Box<dyn RowSource + 'a>,
) -> Box<dyn RowSource + 'a> {
    let collector = CURRENT.with(|cell| cell.borrow().clone());
    match collector {
        Some(c) => Box::new(MeteredSource {
            inner,
            op_id,
            collector: c,
        }),
        None => inner,
    }
}

struct MeteredSource<'a> {
    inner: Box<dyn RowSource + 'a>,
    op_id: PhysicalNodeId,
    collector: Arc<MetricsCollector>,
}

impl<'a> RowSource for MeteredSource<'a> {
    fn next_row(&mut self) -> ExecResult<Option<Row>> {
        let t0 = Instant::now();
        let result = self.inner.next_row();
        let elapsed_ns = t0.elapsed().as_nanos().min(u128::from(u64::MAX)) as u64;
        let produced = matches!(&result, Ok(Some(_)));
        self.collector.record(self.op_id, elapsed_ns, produced);
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn current() -> Option<Arc<MetricsCollector>> {
        CURRENT.with(|cell| cell.borrow().clone())
    }

    #[test]
    fn nested_guards_restore_the_outer_collector() {
        let outer = Arc::new(MetricsCollector::default());
        let inner = Arc::new(MetricsCollector::default());
        let outer_guard = CollectorGuard::install(outer.clone());
        {
            let _inner_guard = CollectorGuard::install(inner.clone());
            assert!(Arc::ptr_eq(&current().unwrap(), &inner));
        }
        assert!(Arc::ptr_eq(&current().unwrap(), &outer));
        drop(outer_guard);
        assert!(current().is_none());
    }
}
