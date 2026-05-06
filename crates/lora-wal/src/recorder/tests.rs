use std::path::Path;
use std::sync::Arc;

use lora_store::{GraphStorageMut, InMemoryGraph, MutationEvent, Properties, PropertyValue};

use super::errors::WroteCommit;
use super::recorder::WalRecorder;
use crate::config::SyncMode;
use crate::errors::WalError;
use crate::lsn::Lsn;
use crate::testing::TmpDir;
use crate::Wal;

fn open_wal(dir: &Path) -> Arc<Wal> {
    let (wal, replay) = Wal::open(dir, SyncMode::PerCommit, 8 * 1024 * 1024, Lsn::ZERO).unwrap();
    assert!(replay.is_empty());
    wal
}

#[test]
fn record_outside_arm_poisons() {
    let dir = TmpDir::new("no-arm");
    let recorder = WalRecorder::new(open_wal(&dir.path));
    lora_store::MutationRecorder::record(&recorder, MutationEvent::Clear);
    assert!(recorder.is_poisoned());
    let msg = lora_store::MutationRecorder::poisoned(&recorder).unwrap();
    assert!(msg.contains("outside an armed query"));
}

#[test]
fn arm_record_commit_round_trip_via_in_memory_graph() {
    let dir = TmpDir::new("happy");
    let recorder: Arc<WalRecorder> = Arc::new(WalRecorder::new(open_wal(&dir.path)));

    let mut g = InMemoryGraph::new();
    g.set_mutation_recorder(Some(recorder.clone()));

    recorder.arm().unwrap();
    let mut props = Properties::new();
    props.insert("v".into(), PropertyValue::Int(1));
    g.create_node(vec!["N".into()], props);
    let mut props2 = Properties::new();
    props2.insert("v".into(), PropertyValue::Int(2));
    g.create_node(vec!["N".into()], props2);
    let outcome = recorder.commit().unwrap();
    assert_eq!(outcome, WroteCommit::Yes);

    assert!(!recorder.is_poisoned());

    // Drop every recorder clone before re-opening the directory,
    // otherwise we'd race with our own live WAL handle.
    g.set_mutation_recorder(None);
    drop(recorder);

    let (_wal, events) =
        Wal::open(&dir.path, SyncMode::PerCommit, 8 * 1024 * 1024, Lsn::ZERO).unwrap();
    assert_eq!(events.len(), 2);
    assert!(matches!(events[0], MutationEvent::CreateNode { id: 0, .. }));
    assert!(matches!(events[1], MutationEvent::CreateNode { id: 1, .. }));
}

#[test]
fn commit_events_records_buffered_transaction_as_one_commit() {
    let dir = TmpDir::new("buffered-events");
    let recorder = WalRecorder::new(open_wal(&dir.path));

    let outcome = recorder
        .commit_events(vec![
            MutationEvent::CreateNode {
                id: 0,
                labels: vec!["N".into()],
                properties: Properties::new(),
            },
            MutationEvent::SetNodeProperty {
                node_id: 0,
                key: "v".into(),
                value: PropertyValue::Int(42),
            },
        ])
        .unwrap();
    assert_eq!(outcome, WroteCommit::Yes);

    drop(recorder);
    let (_wal, events) =
        Wal::open(&dir.path, SyncMode::PerCommit, 8 * 1024 * 1024, Lsn::ZERO).unwrap();
    assert_eq!(events.len(), 2);
    assert!(matches!(events[0], MutationEvent::CreateNode { id: 0, .. }));
    assert!(matches!(
        events[1],
        MutationEvent::SetNodeProperty { node_id: 0, .. }
    ));
}

#[test]
fn arm_then_commit_with_no_mutations_writes_nothing() {
    let dir = TmpDir::new("ro");
    let recorder = WalRecorder::new(open_wal(&dir.path));

    // Simulate a read-only query: arm + commit without any
    // intervening `record` calls.
    let next_before = recorder.wal().next_lsn();
    recorder.arm().unwrap();
    let outcome = recorder.commit().unwrap();
    assert_eq!(outcome, WroteCommit::No);
    let next_after = recorder.wal().next_lsn();
    assert_eq!(
        next_before, next_after,
        "read-only commit must not allocate any LSNs"
    );
}

#[test]
fn abort_drops_in_flight_events_on_replay() {
    let dir = TmpDir::new("abort");
    let recorder: Arc<WalRecorder> = Arc::new(WalRecorder::new(open_wal(&dir.path)));

    let mut g = InMemoryGraph::new();
    g.set_mutation_recorder(Some(recorder.clone()));

    // Tx 1 commits.
    recorder.arm().unwrap();
    g.create_node(vec!["A".into()], Properties::new());
    let _ = recorder.commit().unwrap();
    recorder.flush().unwrap();

    // Tx 2 aborts: the in-memory mutation already happened (the
    // engine has no rollback) but the WAL marks it aborted, so
    // recovery from a fresh process must skip it.
    recorder.arm().unwrap();
    g.create_node(vec!["B".into()], Properties::new());
    let aborted = recorder.abort().unwrap();
    assert!(aborted, "abort after buffered mutations should quarantine");
    recorder.flush().unwrap();

    g.set_mutation_recorder(None);
    drop(recorder);

    let (_wal, events) =
        Wal::open(&dir.path, SyncMode::PerCommit, 8 * 1024 * 1024, Lsn::ZERO).unwrap();
    assert_eq!(events.len(), 1);
    if let MutationEvent::CreateNode { labels, .. } = &events[0] {
        assert_eq!(labels, &vec!["A".to_string()]);
    } else {
        panic!("expected CreateNode for label A, got {:?}", events[0]);
    }
}

#[test]
fn arm_while_armed_poisons() {
    let dir = TmpDir::new("double-arm");
    let recorder = WalRecorder::new(open_wal(&dir.path));
    recorder.arm().unwrap();
    let err = recorder.arm().unwrap_err();
    assert!(matches!(err, WalError::Poisoned));
    assert!(recorder.is_poisoned());
}

#[test]
fn poisoned_recorder_swallows_subsequent_records() {
    let dir = TmpDir::new("swallow");
    let recorder = WalRecorder::new(open_wal(&dir.path));

    // Poison it.
    lora_store::MutationRecorder::record(&recorder, MutationEvent::Clear);
    assert!(recorder.is_poisoned());

    // After poisoning, further `record` calls must NOT touch the
    // WAL or panic — they're a no-op so the engine can finish
    // unwinding before the host observes `poisoned()` and fails
    // the query.
    for _ in 0..10 {
        lora_store::MutationRecorder::record(&recorder, MutationEvent::Clear);
    }
    assert!(recorder.is_poisoned());
}

#[test]
fn checkpoint_marker_through_recorder() {
    let dir = TmpDir::new("ckpt");
    let recorder = WalRecorder::new(open_wal(&dir.path));

    recorder.arm().unwrap();
    lora_store::MutationRecorder::record(&recorder, MutationEvent::Clear);
    assert_eq!(recorder.commit().unwrap(), WroteCommit::Yes);
    recorder.force_fsync().unwrap();
    let snapshot_lsn = recorder.wal().durable_lsn();

    // Exercise the marker path via the recorder's shim after a
    // real durable fence exists.
    let marker_lsn = recorder.checkpoint_marker(snapshot_lsn).unwrap();
    recorder.force_fsync().unwrap();
    assert!(marker_lsn >= Lsn::new(1));

    let outcome = crate::replay::replay_dir(&dir.path, Lsn::ZERO).unwrap();
    assert_eq!(outcome.checkpoint_lsn_observed, Some(snapshot_lsn));
}
