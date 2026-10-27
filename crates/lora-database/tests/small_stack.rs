//! Queries at every shape limit at once must not overflow a small thread
//! stack. A stack overflow aborts the whole process, so the engine moves
//! deep recursion onto fresh stack segments (`lora_ast::stack`) instead of
//! requiring a large stack from the host thread.

#[path = "support/deep_queries.rs"]
mod deep_queries;

use std::sync::Mutex;

use lora_database::{Database, InMemoryGraph};

use deep_queries::{deep_queries, setup, DeepQuery};

/// Stack of a musl thread, the smallest a host is likely to call in on.
const SMALL_STACK: usize = 128 << 10;

/// The segment size is process-wide: tests that change it run one at a
/// time.
static SEGMENT_SIZE: Mutex<()> = Mutex::new(());

fn graph() -> Database<InMemoryGraph> {
    let db = Database::in_memory();
    for query in setup() {
        db.execute(&query, None)
            .unwrap_or_else(|e| panic!("{query}: {e}"));
    }
    db
}

/// Run `case` through every entry point on a thread with `stack` bytes of
/// stack. Only an overflow, which kills the test process, fails.
fn run_on_small_stack(db: &Database<InMemoryGraph>, case: &DeepQuery, stack: usize) {
    let params = case.param_copies();
    std::thread::scope(|scope| {
        std::thread::Builder::new()
            .name(case.name.clone())
            .stack_size(stack)
            .spawn_scoped(scope, || case.run(db, params))
            .unwrap()
            .join()
            .unwrap_or_else(|_| panic!("`{}` panicked", case.name));
    });
}

#[test]
fn deep_queries_run_on_a_small_thread_stack() {
    let _lock = SEGMENT_SIZE.lock().unwrap_or_else(|e| e.into_inner());
    let db = graph();
    for case in deep_queries() {
        run_on_small_stack(&db, &case, SMALL_STACK);
    }
}

/// With segments no larger than a guard's red zone, any recursion that
/// runs further than the red zone between two guards overflows: every
/// recursive path at the limits must pass through a guard often enough.
#[test]
fn every_deep_path_passes_through_a_stack_guard() {
    let _lock = SEGMENT_SIZE.lock().unwrap_or_else(|e| e.into_inner());
    let db = graph();
    lora_ast::stack::set_segment_size(lora_ast::stack::RED_ZONE);
    for case in deep_queries() {
        run_on_small_stack(&db, &case, SMALL_STACK);
    }
    lora_ast::stack::set_segment_size(0);
}
