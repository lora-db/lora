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
    Wal::open(
        dir,
        SyncMode::GroupSync {
            interval_ms: 60_000,
        },
        8 * 1024 * 1024,
        Lsn::ZERO,
    )
    .unwrap()
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

    match Wal::open(
        &dir.path,
        SyncMode::GroupSync {
            interval_ms: 60_000,
        },
        8 * 1024 * 1024,
        Lsn::ZERO,
    ) {
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
    let (wal, _) = Wal::open(
        &dir.path,
        SyncMode::GroupSync {
            interval_ms: 60_000,
        },
        256,
        Lsn::ZERO,
    )
    .unwrap();

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
    let (_, replay) = Wal::open(
        &dir.path,
        SyncMode::GroupSync {
            interval_ms: 60_000,
        },
        8 * 1024 * 1024,
        commit_a,
    )
    .unwrap();
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

    let err = match Wal::open(
        &dir.path,
        SyncMode::GroupSync {
            interval_ms: 60_000,
        },
        8 * 1024 * 1024,
        Lsn::ZERO,
    ) {
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

    let err = match Wal::open(
        &dir.path,
        SyncMode::GroupSync {
            interval_ms: 60_000,
        },
        8 * 1024 * 1024,
        Lsn::ZERO,
    ) {
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
        SyncMode::GroupSync { interval_ms: 25 },
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
fn group_sync_flush_does_not_advance_durable_lsn() {
    let dir = TmpDir::new("group-sync-durable");
    let (wal, _) = Wal::open(
        &dir.path,
        SyncMode::GroupSync {
            interval_ms: 60_000,
        },
        8 * 1024 * 1024,
        Lsn::ZERO,
    )
    .unwrap();

    let begin = wal.begin().unwrap();
    wal.append(begin, &ev(1)).unwrap();
    wal.commit(begin).unwrap();
    wal.flush().unwrap();

    assert_eq!(wal.durable_lsn(), Lsn::ZERO);
    wal.force_fsync().unwrap();
    assert_eq!(wal.durable_lsn().raw(), wal.next_lsn().raw() - 1);
}

#[test]
fn force_fsync_always_advances_durable_lsn() {
    let dir = TmpDir::new("force-fsync");
    let (wal, _) = Wal::open(
        &dir.path,
        SyncMode::GroupSync {
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
    let (wal, _) = Wal::open(
        &dir.path,
        SyncMode::GroupSync {
            interval_ms: 60_000,
        },
        64,
        Lsn::ZERO,
    )
    .unwrap();

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
    let (_, replay) = Wal::open(
        &dir.path,
        SyncMode::GroupSync {
            interval_ms: 60_000,
        },
        64,
        last_commit,
    )
    .unwrap();
    // Everything was at or below the fence, so replay is empty.
    assert!(replay.is_empty());
}

// ---------------------------------------------------------------------
// Crash and corruption recovery.
// ---------------------------------------------------------------------

fn open_with(dir: &Path, target: u64, checkpoint_lsn: Lsn) -> Result<Arc<Wal>, WalError> {
    Wal::open(
        dir,
        SyncMode::GroupSync {
            interval_ms: 60_000,
        },
        target,
        checkpoint_lsn,
    )
    .map(|(wal, _)| wal)
}

fn replay_with(
    dir: &Path,
    target: u64,
    checkpoint_lsn: Lsn,
) -> Result<Vec<MutationEvent>, WalError> {
    Wal::open(
        dir,
        SyncMode::GroupSync {
            interval_ms: 60_000,
        },
        target,
        checkpoint_lsn,
    )
    .map(|(_, events)| events)
}

/// Write `count` single-event transactions through `commit_tx`.
fn write_txs(wal: &Wal, ids: std::ops::Range<u64>) -> Lsn {
    let mut last = Lsn::ZERO;
    for i in ids {
        last = wal.commit_tx_lsn(vec![ev(i)]).unwrap().unwrap();
    }
    last
}

fn segment_paths(dir: &Path) -> Vec<std::path::PathBuf> {
    SegmentDir::new(dir)
        .list()
        .unwrap()
        .into_iter()
        .map(|e| e.path)
        .collect()
}

fn flip_byte(path: &Path, offset: u64) {
    use std::io::{Read, Seek, SeekFrom, Write};
    let mut f = OpenOptions::new()
        .read(true)
        .write(true)
        .open(path)
        .unwrap();
    f.seek(SeekFrom::Start(offset)).unwrap();
    let mut b = [0u8; 1];
    f.read_exact(&mut b).unwrap();
    b[0] ^= 0x10;
    f.seek(SeekFrom::Start(offset)).unwrap();
    f.write_all(&b).unwrap();
    f.sync_all().unwrap();
}

#[test]
fn crash_after_seal_before_next_segment_reopens() {
    let dir = TmpDir::new("crash-seal");
    {
        let (wal, _) = open_default(&dir.path);
        write_txs(&wal, 0..3);
    }
    // Simulate the old rotation order dying between `seal()` and the
    // creation of the next segment: the newest segment is sealed.
    let newest = segment_paths(&dir.path).pop().unwrap();
    let len = fs::metadata(&newest).unwrap().len();
    let mut writer =
        crate::segment::SegmentWriter::open_for_append_at(newest.clone(), len).unwrap();
    writer.seal().unwrap();
    drop(writer);

    let (wal, replay) = open_default(&dir.path);
    assert_eq!(replay, vec![ev(0), ev(1), ev(2)]);
    write_txs(&wal, 3..4);
    drop(wal);
    let (_, replay) = open_default(&dir.path);
    assert_eq!(replay, vec![ev(0), ev(1), ev(2), ev(3)]);
}

#[test]
fn crash_during_segment_create_reopens() {
    for partial in [0usize, 10, 32] {
        let dir = TmpDir::new("crash-create");
        {
            let (wal, _) = open_default(&dir.path);
            write_txs(&wal, 0..3);
        }
        // A next segment whose header never became durable: empty,
        // short, or a full-length header of zeros.
        let next = SegmentDir::new(&dir.path).path_for(crate::dir::SegmentId::new(2));
        fs::write(&next, vec![0u8; partial]).unwrap();

        let (wal, replay) = open_default(&dir.path);
        assert_eq!(replay, vec![ev(0), ev(1), ev(2)], "partial = {partial}");
        write_txs(&wal, 3..4);
        drop(wal);
        let (_, replay) = open_default(&dir.path);
        assert_eq!(
            replay,
            vec![ev(0), ev(1), ev(2), ev(3)],
            "partial = {partial}"
        );
    }
}

#[test]
fn rotation_survives_crash_between_create_and_seal() {
    let dir = TmpDir::new("rotate-order");
    {
        let wal = open_with(&dir.path, 128, Lsn::ZERO).unwrap();
        write_txs(&wal, 0..6);
        assert!(wal.active_segment_id() >= 2);
    }
    // Unseal every segment, as if each rotation died after creating the
    // next segment but before sealing the previous one.
    for path in segment_paths(&dir.path) {
        let mut reader = crate::segment::SegmentReader::open(&path).unwrap();
        while reader.read_record().unwrap().is_some() {}
        let header = crate::segment::SegmentHeader::new(reader.header().base_lsn);
        use std::io::{Seek, SeekFrom, Write};
        let mut f = OpenOptions::new().write(true).open(&path).unwrap();
        f.seek(SeekFrom::Start(0)).unwrap();
        f.write_all(&header.encode()).unwrap();
    }
    let events = replay_with(&dir.path, 128, Lsn::ZERO).unwrap();
    assert_eq!(events, (0..6).map(ev).collect::<Vec<_>>());
}

#[test]
fn mid_log_bit_flip_fails_open_without_truncating() {
    let dir = TmpDir::new("bitflip");
    {
        let (wal, _) = open_default(&dir.path);
        write_txs(&wal, 0..100);
    }
    let path = segment_paths(&dir.path).pop().unwrap();
    let len = fs::metadata(&path).unwrap().len();
    flip_byte(&path, len / 2);

    let err = replay_with(&dir.path, 8 * 1024 * 1024, Lsn::ZERO).unwrap_err();
    assert!(
        err.to_string().contains("corrupt"),
        "expected a corruption error, got {err}"
    );
    assert_eq!(
        fs::metadata(&path).unwrap().len(),
        len,
        "file must not be cut"
    );
}

#[test]
fn corrupt_length_prefix_mid_log_fails_open() {
    let dir = TmpDir::new("bad-len");
    {
        let (wal, _) = open_default(&dir.path);
        write_txs(&wal, 0..20);
    }
    let path = segment_paths(&dir.path).pop().unwrap();
    let len = fs::metadata(&path).unwrap().len();
    // The first record starts right after the 32-byte header. Setting the
    // high byte of its length makes it run past end-of-file.
    flip_byte(&path, 32 + 3);
    let err = replay_with(&dir.path, 8 * 1024 * 1024, Lsn::ZERO).unwrap_err();
    assert!(
        err.to_string().contains("corrupt"),
        "expected a corruption error, got {err}"
    );
    assert_eq!(fs::metadata(&path).unwrap().len(), len);
}

#[test]
fn corruption_in_sealed_segment_fails_open() {
    let dir = TmpDir::new("sealed-corrupt");
    {
        let wal = open_with(&dir.path, 256, Lsn::ZERO).unwrap();
        write_txs(&wal, 0..20);
        assert!(wal.active_segment_id() >= 3);
    }
    let first = segment_paths(&dir.path).remove(0);
    let len = fs::metadata(&first).unwrap().len();
    flip_byte(&first, len - 2);
    let err = replay_with(&dir.path, 256, Lsn::ZERO).unwrap_err();
    assert!(
        err.to_string().contains("corrupt"),
        "expected a corruption error, got {err}"
    );
    assert_eq!(fs::metadata(&first).unwrap().len(), len);
}

#[test]
fn torn_tail_bytes_are_kept_beside_the_segment() {
    let dir = TmpDir::new("torn-keep");
    {
        let (wal, _) = open_default(&dir.path);
        write_txs(&wal, 0..3);
    }
    let path = segment_paths(&dir.path).pop().unwrap();
    let good_len = fs::metadata(&path).unwrap().len();
    {
        use std::io::Write;
        let mut f = OpenOptions::new().append(true).open(&path).unwrap();
        f.write_all(&[0xab; 40]).unwrap();
    }
    let (_, replay) = open_default(&dir.path);
    assert_eq!(replay, vec![ev(0), ev(1), ev(2)]);
    assert_eq!(fs::metadata(&path).unwrap().len(), good_len);
    let kept: Vec<_> = fs::read_dir(&dir.path)
        .unwrap()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_name().to_string_lossy().contains(".torn-"))
        .collect();
    assert_eq!(kept.len(), 1, "torn bytes should be preserved");
    assert_eq!(fs::read(kept[0].path()).unwrap(), vec![0xab; 40]);
}

#[test]
fn replay_rejects_gap_after_truncated_segments() {
    let dir = TmpDir::new("gap");
    let fence;
    {
        let wal = open_with(&dir.path, 128, Lsn::ZERO).unwrap();
        fence = write_txs(&wal, 0..10);
        wal.truncate_up_to(fence).unwrap();
        write_txs(&wal, 10..12);
    }
    // The first segments are gone; without the snapshot that covers them
    // the log is not a complete history.
    let err = replay_with(&dir.path, 128, Lsn::ZERO).unwrap_err();
    assert!(
        err.to_string().contains("gap"),
        "expected a gap error, got {err}"
    );
    // With the covering fence it opens and replays only the tail.
    let events = replay_with(&dir.path, 128, fence).unwrap();
    assert_eq!(events, vec![ev(10), ev(11)]);
}

#[test]
fn replay_rejects_missing_middle_segment() {
    let dir = TmpDir::new("gap-middle");
    {
        let wal = open_with(&dir.path, 128, Lsn::ZERO).unwrap();
        write_txs(&wal, 0..10);
        assert!(wal.active_segment_id() >= 4);
    }
    let paths = segment_paths(&dir.path);
    fs::remove_file(&paths[1]).unwrap();
    let err = replay_with(&dir.path, 128, Lsn::ZERO).unwrap_err();
    assert!(
        err.to_string().contains("gap"),
        "expected a gap error, got {err}"
    );
}

#[test]
fn fresh_wal_starts_above_the_checkpoint_fence() {
    let dir = TmpDir::new("fresh-fence");
    {
        let wal = open_with(&dir.path, 8 * 1024 * 1024, Lsn::new(500)).unwrap();
        assert_eq!(wal.next_lsn(), Lsn::new(501));
        write_txs(&wal, 0..1);
    }
    let events = replay_with(&dir.path, 8 * 1024 * 1024, Lsn::new(500)).unwrap();
    assert_eq!(events, vec![ev(0)]);
}

#[test]
fn reopen_with_fence_past_log_end_never_reuses_fenced_lsns() {
    let dir = TmpDir::new("fence-past-end");
    {
        let (wal, _) = open_default(&dir.path);
        write_txs(&wal, 0..2);
    }
    // A snapshot fenced beyond the surviving log (the tail was lost).
    {
        let wal = open_with(&dir.path, 8 * 1024 * 1024, Lsn::new(100)).unwrap();
        assert_eq!(wal.next_lsn(), Lsn::new(101));
        write_txs(&wal, 2..3);
    }
    let events = replay_with(&dir.path, 8 * 1024 * 1024, Lsn::new(100)).unwrap();
    assert_eq!(events, vec![ev(2)]);
}

fn nested_list(depth: usize) -> PropertyValue {
    let mut value = PropertyValue::Null;
    for _ in 0..depth {
        value = PropertyValue::List(vec![value]);
    }
    value
}

#[test]
fn failed_encode_leaves_the_wal_usable() {
    let dir = TmpDir::new("encode-fail");
    let (wal, _) = open_default(&dir.path);
    write_txs(&wal, 0..1);
    let next = wal.next_lsn();
    let deep = MutationEvent::SetNodeProperty {
        node_id: 0,
        key: "deep".into(),
        value: nested_list(10_000),
    };
    // Run on a big stack so the baseline (recursive, uncapped encoder)
    // reaches the assertion instead of overflowing.
    let wal2 = Arc::clone(&wal);
    let result = std::thread::Builder::new()
        .stack_size(512 * 1024 * 1024)
        .spawn(move || wal2.commit_tx(vec![ev(1), deep]).map(|_| ()))
        .unwrap()
        .join()
        .unwrap();
    assert!(result.is_err(), "too-deep value must be rejected");
    assert_eq!(
        wal.next_lsn(),
        next,
        "a failed encode must not consume LSNs"
    );
    write_txs(&wal, 2..3);
    drop(wal);
    let (_, replay) = open_default(&dir.path);
    assert_eq!(replay, vec![ev(0), ev(2)]);
}

#[test]
fn large_transaction_is_split_into_several_batches() {
    let dir = TmpDir::new("split");
    {
        let (wal, _) = open_default(&dir.path);
        wal.set_batch_target_bytes_for_tests(1024);
        let events: Vec<_> = (0..200).map(ev).collect();
        wal.commit_tx(events).unwrap();
        write_txs(&wal, 200..201);
    }
    let path = segment_paths(&dir.path).pop().unwrap();
    let mut reader = crate::segment::SegmentReader::open(&path).unwrap();
    let mut batches = 0;
    while let Some(record) = reader.read_record().unwrap() {
        if let crate::record::WalRecord::MutationBatch { .. } = record {
            batches += 1;
        }
    }
    assert!(
        batches > 3,
        "expected the tx to span several batches, got {batches}"
    );
    let (_, replay) = open_default(&dir.path);
    assert_eq!(replay, (0..201).map(ev).collect::<Vec<_>>());
    let mut reader =
        crate::history::CommittedTxReader::open(&dir.path, Lsn::ZERO, Lsn::new(u64::MAX)).unwrap();
    let tx = reader.next_tx().unwrap().unwrap();
    assert_eq!(tx.events.len(), 200);
}

#[test]
fn idle_force_fsync_skips_the_fsync() {
    let dir = TmpDir::new("idle-sync");
    let (wal, _) = open_default(&dir.path);
    write_txs(&wal, 0..1);
    wal.force_fsync().unwrap();
    let after_first = wal.fsync_count();
    wal.force_fsync().unwrap();
    wal.force_fsync().unwrap();
    assert_eq!(wal.fsync_count(), after_first, "nothing new was written");
    write_txs(&wal, 1..2);
    wal.force_fsync().unwrap();
    assert_eq!(wal.fsync_count(), after_first + 1);
    assert_eq!(wal.durable_lsn().raw(), wal.next_lsn().raw() - 1);
}

#[test]
fn newest_checkpoint_marker_matches_full_replay() {
    let dir = TmpDir::new("ckpt-scan");
    assert_eq!(
        crate::replay::newest_checkpoint_marker(&dir.path).unwrap(),
        None
    );
    {
        let wal = open_with(&dir.path, 256, Lsn::ZERO).unwrap();
        let first = write_txs(&wal, 0..5);
        wal.checkpoint_marker(first).unwrap();
        let second = write_txs(&wal, 5..12);
        wal.checkpoint_marker(second).unwrap();
        write_txs(&wal, 12..14);
    }
    let scanned = crate::replay::newest_checkpoint_marker(&dir.path).unwrap();
    let replayed = crate::replay::replay_dir(&dir.path, Lsn::ZERO)
        .unwrap()
        .checkpoint_lsn_observed;
    assert!(scanned.is_some());
    assert_eq!(scanned, replayed);
}

fn open_truncating_corruption(dir: &Path, target: u64) -> (Arc<Wal>, Vec<MutationEvent>) {
    let mut collected = crate::replay::CollectEvents::default();
    let wal = Wal::open_replaying_with(
        dir,
        SyncMode::GroupSync {
            interval_ms: 60_000,
        },
        target,
        Lsn::ZERO,
        true,
        &mut |step| collected.step(step),
    )
    .unwrap();
    (wal, collected.events)
}

fn files_containing(dir: &Path, needle: &str) -> Vec<std::path::PathBuf> {
    let mut found: Vec<_> = fs::read_dir(dir)
        .unwrap()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_name().to_string_lossy().contains(needle))
        .map(|e| e.path())
        .collect();
    found.sort();
    found
}

/// `events` is `ev(0)..ev(k)` for some `k < total`; returns `k`.
fn assert_strict_prefix(events: &[MutationEvent], total: u64) -> u64 {
    let k = events.len() as u64;
    assert!(k < total, "replay should stop before the corruption");
    assert_eq!(events, (0..k).map(ev).collect::<Vec<_>>());
    k
}

#[test]
fn truncate_corrupt_tail_cuts_a_mid_log_bit_flip() {
    let dir = TmpDir::new("cut-bitflip");
    {
        let (wal, _) = open_default(&dir.path);
        write_txs(&wal, 0..100);
    }
    let path = segment_paths(&dir.path).pop().unwrap();
    let original = fs::read(&path).unwrap();
    flip_byte(&path, original.len() as u64 / 2);
    let damaged = fs::read(&path).unwrap();

    // Without the option the open still refuses.
    assert!(matches!(
        replay_with(&dir.path, 8 * 1024 * 1024, Lsn::ZERO),
        Err(WalError::Corrupt { .. })
    ));
    assert_eq!(fs::read(&path).unwrap(), damaged);

    let (wal, events) = open_truncating_corruption(&dir.path, 8 * 1024 * 1024);
    let k = assert_strict_prefix(&events, 100);
    let cut = wal.corrupt_cut().expect("the open reports its cut").clone();
    assert_eq!(cut.segment_path, path);
    assert!(cut.moved_segments.is_empty());
    let kept = cut.kept_tail.clone().expect("cut bytes are kept");
    assert_eq!(files_containing(&dir.path, ".corrupt-"), vec![kept.clone()]);
    let offset = cut.offset as usize;
    assert_eq!(fs::read(&kept).unwrap(), damaged[offset..].to_vec());
    assert_eq!(fs::read(&path).unwrap(), damaged[..offset].to_vec());

    write_txs(&wal, 1000..1001);
    drop(wal);

    // A plain reopen finds an intact log: the prefix and the new write.
    let (wal, events) = open_default(&dir.path);
    assert!(wal.corrupt_cut().is_none());
    let mut expected: Vec<_> = (0..k).map(ev).collect();
    expected.push(ev(1000));
    assert_eq!(events, expected);
}

#[test]
fn truncate_corrupt_tail_in_a_sealed_segment_moves_later_segments_aside() {
    let dir = TmpDir::new("cut-sealed");
    {
        let wal = open_with(&dir.path, 256, Lsn::ZERO).unwrap();
        write_txs(&wal, 0..20);
        assert!(wal.active_segment_id() >= 3);
    }
    let segments_before = segment_paths(&dir.path);
    let first = segments_before[0].clone();
    let len = fs::metadata(&first).unwrap().len();
    flip_byte(&first, len - 2);
    let later_bytes: Vec<Vec<u8>> = segments_before[1..]
        .iter()
        .map(|p| fs::read(p).unwrap())
        .collect();

    assert!(matches!(
        replay_with(&dir.path, 256, Lsn::ZERO),
        Err(WalError::Corrupt { .. })
    ));

    let (wal, events) = open_truncating_corruption(&dir.path, 256);
    let k = assert_strict_prefix(&events, 20);
    let cut = wal.corrupt_cut().expect("the open reports its cut").clone();
    assert_eq!(cut.segment_path, first);
    assert!(cut.kept_tail.is_some());
    // Every later segment is kept, byte for byte, under a new name.
    assert_eq!(cut.moved_segments.len(), segments_before.len() - 1);
    for ((from, to), bytes) in cut.moved_segments.iter().zip(&later_bytes) {
        assert_ne!(from, to);
        assert_eq!(&fs::read(to).unwrap(), bytes);
    }
    // The sealed segment is the newest left, so the open starts a fresh
    // second segment in place of the one moved aside.
    let segments_after = segment_paths(&dir.path);
    assert_eq!(segments_after, segments_before[..2].to_vec());
    assert!(fs::metadata(&segments_after[1]).unwrap().len() < later_bytes[0].len() as u64);

    write_txs(&wal, 1000..1001);
    drop(wal);

    let (wal, events) = open_default(&dir.path);
    assert!(wal.corrupt_cut().is_none());
    let mut expected: Vec<_> = (0..k).map(ev).collect();
    expected.push(ev(1000));
    assert_eq!(events, expected);
}

/// A node event whose WAL record is about 64 KiB.
fn big_ev(id: u64) -> MutationEvent {
    let mut p = Properties::new();
    p.insert("blob".into(), PropertyValue::String("x".repeat(64 * 1024)));
    MutationEvent::CreateNode {
        id,
        labels: vec!["N".into()],
        properties: p,
    }
}

/// Events for twice the replay buffer.
const BIG_TX_EVENTS: u64 = (2 * crate::PENDING_BUDGET_BYTES / (64 * 1024)) as u64;

#[derive(Debug, PartialEq)]
enum Step {
    Commit(usize),
    Partial(usize),
    Discard,
}

/// Replay `dir`, recording the steps the sink sees and the events it
/// keeps.
fn replay_steps(dir: &Path) -> (Vec<Step>, Vec<MutationEvent>) {
    let mut steps = Vec::new();
    let mut collected = crate::replay::CollectEvents::default();
    Wal::open_replaying(
        dir,
        SyncMode::GroupSync {
            interval_ms: 60_000,
        },
        8 * 1024 * 1024,
        Lsn::ZERO,
        &mut |step| {
            steps.push(match &step {
                crate::ReplayStep::Commit(events) => Step::Commit(events.len()),
                crate::ReplayStep::Partial(events) => Step::Partial(events.len()),
                crate::ReplayStep::Discard => Step::Discard,
            });
            collected.step(step)
        },
    )
    .unwrap();
    (steps, collected.events)
}

/// Collapse runs of partial steps into one, summing their events.
fn merge_partials(steps: Vec<Step>) -> Vec<Step> {
    let mut merged: Vec<Step> = Vec::new();
    for step in steps {
        match (merged.last_mut(), step) {
            (Some(Step::Partial(total)), Step::Partial(n)) => *total += n,
            (_, step) => merged.push(step),
        }
    }
    merged
}

#[test]
fn a_transaction_past_the_replay_buffer_reaches_the_sink_in_parts() {
    let dir = TmpDir::new("big-commit");
    let big: Vec<_> = (0..BIG_TX_EVENTS).map(big_ev).collect();
    {
        let (wal, _) = open_default(&dir.path);
        write_txs(&wal, 0..1);
        wal.commit_tx(big.clone()).unwrap();
        write_txs(&wal, 1..2);
    }
    let (steps, events) = replay_steps(&dir.path);
    let merged = merge_partials(steps);
    let Step::Partial(streamed) = merged[1] else {
        panic!("expected the large transaction in parts, got {merged:?}");
    };
    assert_eq!(merged[0], Step::Commit(1));
    assert_eq!(
        merged[2],
        Step::Commit(BIG_TX_EVENTS as usize - streamed),
        "the commit carries the rest"
    );
    assert_eq!(merged[3], Step::Commit(1));
    assert_eq!(merged.len(), 4);
    let mut expected = vec![ev(0)];
    expected.extend(big);
    expected.push(ev(1));
    assert_eq!(events, expected);
}

#[test]
fn a_large_transaction_without_its_commit_is_withdrawn() {
    let dir = TmpDir::new("big-uncommitted");
    {
        let (wal, _) = open_default(&dir.path);
        write_txs(&wal, 0..1);
        let begin = wal.begin().unwrap();
        for id in 1..=BIG_TX_EVENTS {
            wal.append(begin, &big_ev(id)).unwrap();
        }
        wal.flush().unwrap();
    }
    let (steps, events) = replay_steps(&dir.path);
    let merged = merge_partials(steps);
    assert_eq!(merged.first(), Some(&Step::Commit(1)));
    assert!(matches!(merged[1], Step::Partial(_)), "{merged:?}");
    assert_eq!(merged.last(), Some(&Step::Discard));
    assert_eq!(merged.len(), 3);
    assert_eq!(events, vec![ev(0)]);
}

#[test]
fn a_large_transaction_followed_by_another_is_withdrawn_before_it() {
    let dir = TmpDir::new("big-abandoned");
    {
        let (wal, _) = open_default(&dir.path);
        let begin = wal.begin().unwrap();
        for id in 0..BIG_TX_EVENTS {
            wal.append(begin, &big_ev(id)).unwrap();
        }
        // A failed append leaves this shape: the next transaction starts
        // after records of one that never commits.
        write_txs(&wal, 100..101);
        wal.abort(begin).unwrap();
        wal.flush().unwrap();
    }
    let (steps, events) = replay_steps(&dir.path);
    let merged = merge_partials(steps);
    assert!(matches!(merged[0], Step::Partial(_)), "{merged:?}");
    assert_eq!(merged[1..], [Step::Discard, Step::Commit(1)]);
    assert_eq!(events, vec![ev(100)]);
}

#[test]
fn a_withdrawn_large_transaction_that_commits_fails_replay() {
    let dir = TmpDir::new("big-interleaved");
    {
        let (wal, _) = open_default(&dir.path);
        let begin = wal.begin().unwrap();
        for id in 0..BIG_TX_EVENTS {
            wal.append(begin, &big_ev(id)).unwrap();
        }
        write_txs(&wal, 100..101);
        wal.commit(begin).unwrap();
        wal.flush().unwrap();
    }
    let err = Wal::open(
        &dir.path,
        SyncMode::GroupSync {
            interval_ms: 60_000,
        },
        8 * 1024 * 1024,
        Lsn::ZERO,
    )
    .err()
    .expect("an interleaved large transaction cannot be replayed in parts");
    assert!(
        err.to_string().contains("came between its own"),
        "unexpected error: {err}"
    );
}
