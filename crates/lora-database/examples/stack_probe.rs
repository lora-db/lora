//! Smallest thread stack each query in `tests/support/deep_queries.rs`
//! runs on.
//!
//! `cargo run --release -p lora-database --example stack_probe` bisects
//! the stack size per query, running each attempt in a child process
//! because an overflow aborts it. `... -- <index> <KiB>` runs one query on
//! a thread with that stack and exits 0 when it did not overflow.
//! `STACK_PROBE_SEGMENTS=1` bisects the size of the segments stack guards
//! allocate instead, on a 64 KiB thread: the longest stretch of stack a
//! query uses between two guards.

use std::process::Command;

use lora_database::Database;

#[path = "../tests/support/deep_queries.rs"]
mod deep_queries;

fn run_one(index: usize, kib: usize) {
    let mut cases = deep_queries::deep_queries();
    let case = cases.swap_remove(index);
    let db = Database::in_memory();
    for q in deep_queries::setup() {
        db.execute(&q, None).unwrap();
    }
    // Segment mode: run on a tiny thread and bisect the size of the stack
    // segments guards allocate instead, which finds the longest stretch
    // of stack used between two guards.
    let kib = if std::env::var("STACK_PROBE_SEGMENTS").is_ok() {
        lora_ast::stack::set_segment_size(kib * 1024);
        64
    } else {
        kib
    };
    if std::env::var("STACK_PROBE_STAGE").as_deref() == Ok("exec") {
        // Warm the plan cache on the main thread, so the measured run
        // only executes.
        let _ = db.execute_with_params(&case.query, None, case.params.clone());
    }
    let mut params = case.param_copies();
    std::thread::scope(|scope| {
        std::thread::Builder::new()
            .stack_size(kib * 1024)
            .spawn_scoped(scope, || {
                match std::env::var("STACK_PROBE_STAGE").as_deref() {
                    Ok("parse") => {
                        let _ = lora_database::parse_query(&case.query);
                    }
                    Ok("explain") => {
                        let _ = db.explain(&case.query, params.pop());
                    }
                    Ok("execute") => {
                        let started = std::time::Instant::now();
                        let result = db.execute_with_params(
                            &case.query,
                            None,
                            params.pop().unwrap_or_default(),
                        );
                        if std::env::var("STACK_PROBE_VERBOSE").is_ok() {
                            match result {
                                Ok(_) => eprintln!("ok in {:?}", started.elapsed()),
                                Err(e) => eprintln!("error in {:?}: {e}", started.elapsed()),
                            }
                        }
                    }
                    _ => case.run(&db, params),
                }
            })
            .unwrap()
            .join()
            .unwrap();
    });
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() == 3 {
        run_one(args[1].parse().unwrap(), args[2].parse().unwrap());
        return;
    }
    if args.get(1).map(String::as_str) == Some("list") {
        for (i, case) in deep_queries::deep_queries().iter().enumerate() {
            println!("{i} {}", case.name);
        }
        return;
    }
    let me = std::env::current_exe().unwrap();
    let names: Vec<String> = deep_queries::deep_queries()
        .into_iter()
        .map(|c| c.name)
        .collect();
    let ok = |i: usize, kib: usize| {
        Command::new(&me)
            .args([i.to_string(), kib.to_string()])
            .stderr(std::process::Stdio::null())
            .status()
            .unwrap()
            .success()
    };
    let max_kib: usize = std::env::var("STACK_PROBE_MAX_KIB")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(64 * 1024);
    let mut worst = 0;
    let only = std::env::var("STACK_PROBE_ONLY").ok();
    for (i, name) in names.iter().enumerate() {
        if only.as_deref().is_some_and(|o| !name.contains(o)) {
            continue;
        }
        // Bisect in 8 KiB steps between 16 KiB and the maximum.
        let (mut lo, mut hi) = (2usize, max_kib / 8);
        if !ok(i, hi * 8) {
            println!("{:>8}  {name}", "> max");
            continue;
        }
        while lo < hi {
            let mid = (lo + hi) / 2;
            if ok(i, mid * 8) {
                hi = mid;
            } else {
                lo = mid + 1;
            }
        }
        worst = worst.max(hi * 8);
        println!("{:>6} KiB  {name}", hi * 8);
    }
    println!("worst: {worst} KiB");
}
