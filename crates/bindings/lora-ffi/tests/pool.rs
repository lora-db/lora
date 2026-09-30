//! Concurrency guards for the C ABI, driven the way a C host drives it:
//! from a small fixed pool of threads, with more writers than threads.
//!
//! Every scenario runs under a hard deadline so a regression fails the
//! test instead of hanging the runner.

use std::ffi::{c_char, CStr, CString};
use std::ptr;
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use lora_ffi::*;

const DEADLINE: Duration = Duration::from_secs(30);

#[derive(Clone, Copy)]
struct Db(*mut LoraDatabase);
unsafe impl Send for Db {}
unsafe impl Sync for Db {}

#[derive(Clone, Copy)]
struct Stream(*mut LoraQueryStream);
unsafe impl Send for Stream {}

fn new_db() -> Db {
    let mut db = ptr::null_mut();
    assert_eq!(unsafe { lora_db_new(&mut db) }, 0);
    Db(db)
}

fn take(s: *mut c_char) -> String {
    if s.is_null() {
        return String::new();
    }
    let out = unsafe { CStr::from_ptr(s) }.to_string_lossy().into_owned();
    unsafe { lora_string_free(s) };
    out
}

fn execute(db: Db, query: &str) -> String {
    let q = CString::new(query).unwrap();
    let mut result = ptr::null_mut();
    let mut error = ptr::null_mut();
    let status =
        unsafe { lora_db_execute_json(db.0, q.as_ptr(), ptr::null(), &mut result, &mut error) };
    let error = take(error);
    assert_eq!(status, 0, "{query}: {error}");
    take(result)
}

fn transaction(db: Db, statements: &str) {
    let s = CString::new(statements).unwrap();
    let mut result = ptr::null_mut();
    let mut error = ptr::null_mut();
    let status =
        unsafe { lora_db_transaction_json(db.0, s.as_ptr(), ptr::null(), &mut result, &mut error) };
    let error = take(error);
    assert_eq!(status, 0, "{error}");
    take(result);
}

fn open_stream(db: Db, query: &str) -> Stream {
    let q = CString::new(query).unwrap();
    let mut stream = ptr::null_mut();
    let mut error = ptr::null_mut();
    let status =
        unsafe { lora_db_stream_open_json(db.0, q.as_ptr(), ptr::null(), &mut stream, &mut error) };
    let error = take(error);
    assert_eq!(status, 0, "{query}: {error}");
    Stream(stream)
}

/// Pull one row; `None` at end of stream.
fn next(stream: Stream) -> Option<String> {
    let mut row = ptr::null_mut();
    let mut error = ptr::null_mut();
    let status = unsafe { lora_stream_next_json(stream.0, &mut row, &mut error) };
    let error = take(error);
    assert_eq!(status, 0, "{error}");
    if row.is_null() {
        None
    } else {
        Some(take(row))
    }
}

fn free_stream(stream: Stream) {
    unsafe { lora_stream_free(stream.0) }
}

fn free_db(db: Db) {
    unsafe { lora_db_free(db.0) }
}

fn node_count(db: Db) -> u64 {
    let mut n = 0;
    assert_eq!(unsafe { lora_db_node_count(db.0, &mut n) }, 0);
    n
}

/// Run `work` on its own thread and fail if it has not finished by the
/// deadline (a hung worker is left behind; the test still fails).
fn within_deadline(name: &str, work: impl FnOnce() + Send + 'static) {
    let (done_tx, done_rx) = mpsc::channel();
    thread::spawn(move || {
        work();
        let _ = done_tx.send(());
    });
    match done_rx.recv_timeout(DEADLINE) {
        Ok(()) => {}
        Err(mpsc::RecvTimeoutError::Timeout) => panic!("{name}: hung for {DEADLINE:?}"),
        Err(mpsc::RecvTimeoutError::Disconnected) => panic!("{name}: worker panicked"),
    }
}

/// A fixed pool of `threads` workers draining a job queue, like a C host's
/// thread pool. Returns once every job has run.
fn run_pool(threads: usize, jobs: Vec<Box<dyn FnOnce() + Send>>) {
    let (tx, rx) = mpsc::channel::<Box<dyn FnOnce() + Send>>();
    let rx = Arc::new(Mutex::new(rx));
    for job in jobs {
        tx.send(job).unwrap();
    }
    drop(tx);
    let workers: Vec<_> = (0..threads)
        .map(|_| {
            let rx = rx.clone();
            thread::spawn(move || loop {
                let job = rx.lock().unwrap().recv();
                match job {
                    Ok(job) => job(),
                    Err(_) => return,
                }
            })
        })
        .collect();
    for w in workers {
        w.join().expect("pool worker panicked");
    }
}

/// 2 pool threads, 60 writers: auto-commit writes, batched transactions and
/// mutating streams (each opened, drained and freed within its job). Every
/// call that waits for the writer lock waits for a writer that is running,
/// so the pool always drains.
#[test]
fn writers_outnumbering_pool_threads_all_finish() {
    within_deadline("pool", || {
        let db = new_db();
        let jobs: Vec<Box<dyn FnOnce() + Send>> = (0..60)
            .map(|i| -> Box<dyn FnOnce() + Send> {
                match i % 3 {
                    0 => Box::new(move || {
                        execute(db, &format!("CREATE (:W {{i: {i}}})"));
                    }),
                    1 => Box::new(move || {
                        transaction(
                            db,
                            &format!(
                                r#"[{{"query":"CREATE (:W {{i: {i}}})"}},{{"query":"MATCH (n:W) RETURN count(n)"}}]"#
                            ),
                        );
                    }),
                    _ => Box::new(move || {
                        let s = open_stream(db, &format!("CREATE (n:W {{i: {i}}}) RETURN n.i"));
                        while next(s).is_some() {}
                        free_stream(s);
                    }),
                }
            })
            .collect();
        run_pool(2, jobs);
        assert_eq!(node_count(db), 60);
        free_db(db);
    });
}

/// A mutating stream holds the writer lock until it is exhausted or freed,
/// and a C host may open it, pull from it and free it on three different
/// threads. The lock must be released properly wherever `lora_stream_free`
/// runs: afterwards other threads can write, and the rolled-back and the
/// committed stream behave as documented.
#[test]
fn mutating_stream_can_move_between_threads() {
    within_deadline("stream across threads", || {
        let db = new_db();

        // Opened on one thread, drained on a second, freed on a third:
        // commits.
        let s = thread::spawn(move || {
            open_stream(db, "UNWIND range(1, 3) AS i CREATE (:S {i: i}) RETURN i")
        })
        .join()
        .unwrap();
        let first = thread::spawn(move || next(s)).join().unwrap();
        assert!(first.is_some());
        thread::spawn(move || while next(s).is_some() {})
            .join()
            .unwrap();
        thread::spawn(move || free_stream(s)).join().unwrap();
        assert_eq!(node_count(db), 3);

        // Opened on one thread, freed early on another: rolls back and
        // releases the writer lock.
        let s = thread::spawn(move || {
            open_stream(db, "UNWIND range(1, 3) AS i CREATE (:R {i: i}) RETURN i")
        })
        .join()
        .unwrap();
        thread::spawn(move || {
            next(s);
        })
        .join()
        .unwrap();
        thread::spawn(move || free_stream(s)).join().unwrap();
        assert_eq!(node_count(db), 3);

        // The writer lock is free again, from any thread, many times over.
        let writers: Vec<_> = (0..8)
            .map(|i| thread::spawn(move || execute(db, &format!("CREATE (:After {{i: {i}}})"))))
            .collect();
        for w in writers {
            w.join().unwrap();
        }
        assert_eq!(node_count(db), 11);
        free_db(db);
    });
}

/// `lora_db_free` may run before `lora_stream_free`: the stream keeps the
/// database alive, and the database is released only after the stream (and
/// its writer lock) is gone, on whichever threads the calls run.
#[test]
fn mutating_stream_outlives_freed_database() {
    within_deadline("stream after db free", || {
        for early in [false, true] {
            let db = new_db();
            let s = thread::spawn(move || {
                open_stream(db, "UNWIND range(1, 3) AS i CREATE (:S {i: i}) RETURN i")
            })
            .join()
            .unwrap();
            thread::spawn(move || free_db(db)).join().unwrap();
            thread::spawn(move || {
                if early {
                    next(s);
                } else {
                    while next(s).is_some() {}
                }
            })
            .join()
            .unwrap();
            thread::spawn(move || free_stream(s)).join().unwrap();
        }
    });
}
