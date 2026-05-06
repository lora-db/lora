use std::fs::{self, OpenOptions};
use std::path::Path;
use std::sync::Arc;

use lora_store::{MutationEvent, Properties, PropertyValue};

use super::wal::Wal;
use crate::config::SyncMode;
use crate::dir::SegmentDir;
use crate::errors::WalError;
use crate::lsn::Lsn;
use crate::testing::TmpDir;

fn ev(id: u64) -> MutationEvent {
    let mut p = Properties::new();
    p.insert("v".into(), PropertyValue::Int(id as i64));
    MutationEvent::CreateNode {
        id,
        labels: vec!["N".into()],
        properties: p,
    }
}

fn open_default(dir: &Path) -> (Arc<Wal>, Vec<MutationEvent>) {
    Wal::open(dir, SyncMode::PerCommit, 8 * 1024 * 1024, Lsn::ZERO).unwrap()
}

#[test]
fn fresh_open_creates_first_segment() {
    let dir = TmpDir::new("fresh");
    let (wal, replay) = open_default(&dir.path);
    assert!(replay.is_empty());
    assert_eq!(wal.next_lsn(), Lsn::new(1));
    assert_eq!(wal.active_segment_id(), 1);
    // No CURRENT pointer file is written — the highest segment id
    // is the source of truth for "active segment".
    let entries: Vec<_> = fs::read_dir(&dir.path)
        .unwrap()
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    assert!(
        entries.iter().any(|n| n == ".lora-wal.lock"),
        "WAL dir should contain the live directory lock, found: {entries:?}"
    );
    assert!(
        entries
            .iter()
            .filter(|n| n.as_str() != ".lora-wal.lock")
            .all(|n| n.ends_with(".wal")),
        "WAL dir should contain only segment files plus the lock, found: {entries:?}"
    );
}

#[test]
fn opening_same_directory_twice_fails_until_first_handle_drops() {
    let dir = TmpDir::new("exclusive");
    let (wal, _) = open_default(&dir.path);

    match Wal::open(&dir.path, SyncMode::PerCommit, 8 * 1024 * 1024, Lsn::ZERO) {
        Err(WalError::AlreadyOpen { dir: locked_dir }) => {
            assert_eq!(locked_dir, dir.path);
        }
        Err(err) => panic!("expected AlreadyOpen, got {err:?}"),
        Ok(_) => panic!("second WAL open on same directory should fail"),
    }

    drop(wal);
    let (reopened, _) = open_default(&dir.path);
    drop(reopened);
}

#[test]
fn begin_append_commit_round_trip_through_replay() {
    let dir = TmpDir::new("commit");

    // First boot: write three transactions and crash without
    // running shutdown.
    {
        let (wal, _) = open_default(&dir.path);
        let begin = wal.begin().unwrap();
        wal.append(begin, &ev(1)).unwrap();
        wal.append(begin, &ev(2)).unwrap();
        wal.commit(begin).unwrap();
        wal.flush().unwrap();

        let begin = wal.begin().unwrap();
        wal.append(begin, &ev(3)).unwrap();
        wal.commit(begin).unwrap();
        wal.flush().unwrap();
        // drop without explicit close
    }

    // Second boot: replay should yield events 1, 2, 3 in order.
    let (wal, replay) = open_default(&dir.path);
    assert_eq!(replay.len(), 3);
    assert_eq!(replay[0], ev(1));
    assert_eq!(replay[1], ev(2));
    assert_eq!(replay[2], ev(3));
    // next_lsn should be past every record we wrote (2 begins +
    // 3 mutations + 2 commits = 7 records → next_lsn = 8).
    assert_eq!(wal.next_lsn(), Lsn::new(8));
}

#[test]
fn aborted_transaction_is_dropped_on_replay() {
    let dir = TmpDir::new("abort");

    {
        let (wal, _) = open_default(&dir.path);
        let b1 = wal.begin().unwrap();
        wal.append(b1, &ev(1)).unwrap();
        wal.commit(b1).unwrap();
        wal.flush().unwrap();

        let b2 = wal.begin().unwrap();
        wal.append(b2, &ev(99)).unwrap();
        wal.abort(b2).unwrap();
        wal.flush().unwrap();
    }

    let (_, replay) = open_default(&dir.path);
    assert_eq!(replay, vec![ev(1)]);
}

#[test]
fn uncommitted_transaction_at_end_of_log_is_discarded() {
    let dir = TmpDir::new("uncommitted");

    {
        let (wal, _) = open_default(&dir.path);
        let b1 = wal.begin().unwrap();
        wal.append(b1, &ev(1)).unwrap();
        wal.commit(b1).unwrap();
        wal.flush().unwrap();

        // Begin + append but never commit. Simulates a crash
        // mid-query.
        let b2 = wal.begin().unwrap();
        wal.append(b2, &ev(99)).unwrap();
        wal.flush().unwrap();
    }

    let (_, replay) = open_default(&dir.path);
    assert_eq!(replay, vec![ev(1)]);
}

#[test]
fn segment_rotation_at_begin_boundary() {
    let dir = TmpDir::new("rotate");

    // Tiny segment target so we trip rotation on the second
    // transaction.
    let (wal, _) = Wal::open(&dir.path, SyncMode::PerCommit, 256, Lsn::ZERO).unwrap();

    // First tx: a few events, takes us past 256 bytes.
    let b1 = wal.begin().unwrap();
    for i in 0..5 {
        wal.append(b1, &ev(i)).unwrap();
    }
    wal.commit(b1).unwrap();
    wal.flush().unwrap();
    assert_eq!(wal.active_segment_id(), 1);

    // Second `begin` triggers rotation.
    let b2 = wal.begin().unwrap();
    wal.append(b2, &ev(100)).unwrap();
    wal.commit(b2).unwrap();
    wal.flush().unwrap();
    assert_eq!(
        wal.active_segment_id(),
        2,
        "begin() should have rotated to segment 2"
    );

    let segments = SegmentDir::new(&dir.path).list().unwrap();
    assert_eq!(segments.len(), 2);

    drop(wal);
    let (_, replay) = open_default(&dir.path);
    assert_eq!(replay.len(), 6);
}

#[test]
fn checkpoint_lsn_skips_already_checkpointed_events() {
    let dir = TmpDir::new("ckpt-skip");
    let (wal, _) = open_default(&dir.path);

    // Tx A: events 1,2 — ends at lsn 4.
    let a = wal.begin().unwrap();
    wal.append(a, &ev(1)).unwrap();
    wal.append(a, &ev(2)).unwrap();
    let commit_a = wal.commit(a).unwrap();
    wal.flush().unwrap();

    // Tx B: event 3 — past the fence.
    let b = wal.begin().unwrap();
    wal.append(b, &ev(3)).unwrap();
    wal.commit(b).unwrap();
    wal.flush().unwrap();
    drop(wal);

    // Re-open with checkpoint_lsn = commit_a so tx A is treated
    // as already-applied.
    let (_, replay) = Wal::open(&dir.path, SyncMode::PerCommit, 8 * 1024 * 1024, commit_a).unwrap();
    assert_eq!(replay, vec![ev(3)]);
}

#[test]
fn replay_rejects_commit_without_begin() {
    let dir = TmpDir::new("commit-without-begin");

    {
        let (wal, _) = open_default(&dir.path);
        wal.commit(Lsn::new(99)).unwrap();
        wal.flush().unwrap();
    }

    let err = match Wal::open(&dir.path, SyncMode::PerCommit, 8 * 1024 * 1024, Lsn::ZERO) {
        Ok(_) => panic!("malformed WAL should not open"),
        Err(err) => err,
    };
    assert!(
        matches!(err, WalError::Malformed(ref msg) if msg.contains("missing tx begin")),
        "expected malformed missing-begin error, got {err:?}"
    );
}

#[test]
fn replay_rejects_mutation_without_begin() {
    let dir = TmpDir::new("mutation-without-begin");

    {
        let (wal, _) = open_default(&dir.path);
        wal.append(Lsn::new(99), &ev(1)).unwrap();
        wal.flush().unwrap();
    }

    let err = match Wal::open(&dir.path, SyncMode::PerCommit, 8 * 1024 * 1024, Lsn::ZERO) {
        Ok(_) => panic!("malformed WAL should not open"),
        Err(err) => err,
    };
    assert!(
        matches!(err, WalError::Malformed(ref msg) if msg.contains("missing tx begin")),
        "expected malformed missing-begin error, got {err:?}"
    );
}

#[test]
fn torn_tail_is_truncated_on_open() {
    let dir = TmpDir::new("torn");

    {
        let (wal, _) = open_default(&dir.path);
        let b = wal.begin().unwrap();
        wal.append(b, &ev(1)).unwrap();
        wal.commit(b).unwrap();
        wal.flush().unwrap();
    }

    // Append garbage to the active segment by hand.
    let segments = SegmentDir::new(&dir.path).list().unwrap();
    let active = &segments.last().unwrap().path;
    {
        use std::io::Write;
        let mut f = OpenOptions::new().append(true).open(active).unwrap();
        f.write_all(&[0xff; 32]).unwrap();
        f.sync_all().unwrap();
    }

    // Re-open. Torn tail must be truncated; replay still yields
    // ev(1); next_lsn picks up cleanly.
    let (wal, replay) = open_default(&dir.path);
    assert_eq!(replay, vec![ev(1)]);

    // Subsequent appends don't trip a CRC failure.
    let b = wal.begin().unwrap();
    wal.append(b, &ev(2)).unwrap();
    wal.commit(b).unwrap();
    wal.flush().unwrap();
    drop(wal);

    let (_, replay) = open_default(&dir.path);
    assert_eq!(replay, vec![ev(1), ev(2)]);
}

#[test]
fn checkpoint_marker_is_recorded_and_observed() {
    let dir = TmpDir::new("ckpt-marker");

    let snapshot_lsn = {
        let (wal, _) = open_default(&dir.path);
        let b = wal.begin().unwrap();
        wal.append(b, &ev(1)).unwrap();
        let commit = wal.commit(b).unwrap();
        wal.flush().unwrap();
        wal.checkpoint_marker(commit).unwrap();
        wal.flush().unwrap();
        commit
    };

    let outcome = crate::replay::replay_dir(&dir.path, Lsn::ZERO).unwrap();
    assert_eq!(
        outcome.checkpoint_lsn_observed,
        Some(snapshot_lsn),
        "checkpoint marker should be surfaced by replay"
    );
}

#[test]
fn group_mode_is_cooperative_until_force_fsync() {
    let dir = TmpDir::new("group");
    let (wal, _) = Wal::open(
        &dir.path,
        SyncMode::Group { interval_ms: 25 },
        8 * 1024 * 1024,
        Lsn::ZERO,
    )
    .unwrap();

    let begin = wal.begin().unwrap();
    wal.append(begin, &ev(1)).unwrap();
    wal.commit(begin).unwrap();
    wal.flush().unwrap(); // Group: write_buffer only; durable_lsn untouched.

    assert_eq!(
        wal.durable_lsn(),
        Lsn::ZERO,
        "Group flush() must not advance durable_lsn"
    );

    wal.force_fsync().unwrap();
    assert_eq!(wal.durable_lsn().raw(), wal.next_lsn().raw() - 1);
    drop(wal);
}

#[test]
fn none_mode_advances_durable_lsn_on_flush() {
    let dir = TmpDir::new("none");
    let (wal, _) = Wal::open(&dir.path, SyncMode::None, 8 * 1024 * 1024, Lsn::ZERO).unwrap();

    let begin = wal.begin().unwrap();
    wal.append(begin, &ev(1)).unwrap();
    wal.commit(begin).unwrap();
    wal.flush().unwrap();

    // None mode: flush() advances durable_lsn even without
    // fsync, because the mode opted out of crash durability.
    assert_eq!(wal.durable_lsn().raw(), wal.next_lsn().raw() - 1);
}

#[test]
fn force_fsync_always_advances_durable_lsn() {
    let dir = TmpDir::new("force-fsync");
    let (wal, _) = Wal::open(
        &dir.path,
        SyncMode::Group {
            interval_ms: 60_000,
        },
        8 * 1024 * 1024,
        Lsn::ZERO,
    )
    .unwrap();

    let begin = wal.begin().unwrap();
    wal.append(begin, &ev(1)).unwrap();
    wal.commit(begin).unwrap();
    wal.flush().unwrap(); // Group flush: durable_lsn unchanged.
    assert_eq!(wal.durable_lsn(), Lsn::ZERO);

    // force_fsync bypasses the configured cadence — used by
    // checkpoints to grab a fence on demand.
    wal.force_fsync().unwrap();
    assert_eq!(wal.durable_lsn().raw(), wal.next_lsn().raw() - 1);
}

#[test]
fn truncate_up_to_drops_old_sealed_segments() {
    let dir = TmpDir::new("truncate");

    // Tiny target so each tx forces a rotation on the next begin.
    let (wal, _) = Wal::open(&dir.path, SyncMode::PerCommit, 64, Lsn::ZERO).unwrap();

    let mut last_commit = Lsn::ZERO;
    for i in 0..5 {
        let b = wal.begin().unwrap();
        wal.append(b, &ev(i)).unwrap();
        last_commit = wal.commit(b).unwrap();
        wal.flush().unwrap();
    }
    // Five transactions × tiny target: we should be on segment ≥ 4.
    assert!(
        wal.active_segment_id() >= 4,
        "expected several rotations, got {}",
        wal.active_segment_id()
    );

    let segments = SegmentDir::new(&dir.path);
    let before = segments.list().unwrap().len();
    wal.truncate_up_to(last_commit).unwrap();
    let after = segments.list().unwrap().len();

    assert!(
        after < before,
        "truncate_up_to should have dropped at least one segment ({} → {})",
        before,
        after
    );
    // Active + tombstone are always retained.
    assert!(
        after >= 2,
        "active and the segment preceding it must be kept"
    );

    // Subsequent appends + reopen still produce all five events
    // because the dropped segments only contained transactions
    // already at or below `last_commit`, which we feed back as
    // the checkpoint fence on reopen.
    drop(wal);
    let (_, replay) = Wal::open(&dir.path, SyncMode::PerCommit, 64, last_commit).unwrap();
    // Everything was at or below the fence, so replay is empty.
    assert!(replay.is_empty());
}
