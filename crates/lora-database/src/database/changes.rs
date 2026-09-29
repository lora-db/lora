//! Change-feed entry points on [`Database<InMemoryGraph>`].

use std::any::Any;

use lora_store::{GraphStorage, GraphStorageMut, InMemoryGraph, MutationEvent};
use lora_wal::Lsn;

use crate::changes::{
    publish_committed, ChangeFeed, ChangeFeedOptions, HistoryReplay, HistorySources, PreImages,
    Resume,
};
use crate::database::Database;
use crate::error::LoraError;

impl Database<InMemoryGraph> {
    /// Open a feed of committed changes.
    ///
    /// Without `from_lsn` the feed starts with the next commit. With
    /// `from_lsn` it first replays every batch after that LSN: from the
    /// in-memory window when it is recent enough, otherwise (WAL-backed
    /// databases only) by rebuilding history from the WAL. An LSN the
    /// database no longer retains fails with `LORA_CHANGES_TRUNCATED`.
    ///
    /// The first call turns change capture on for the lifetime of the
    /// database; it waits for the writer lock once so no commit is missed.
    pub fn changes(&self, options: ChangeFeedOptions) -> Result<ChangeFeed, LoraError> {
        if !self.changes.is_active() {
            let _writer = self
                .writer
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            self.activate_changes();
        }
        self.subscribe_changes(options)
    }

    /// Non-blocking [`Self::changes`]: `None` when this is the first feed on
    /// the database and turning capture on would have to wait for a write
    /// in progress. Bindings call it on their event-loop thread and fall
    /// back to [`Self::changes`] on a worker.
    pub fn try_changes(&self, options: ChangeFeedOptions) -> Option<Result<ChangeFeed, LoraError>> {
        if !self.changes.is_active() {
            let _writer = match self.writer.try_lock() {
                Ok(guard) => guard,
                Err(std::sync::TryLockError::Poisoned(poisoned)) => poisoned.into_inner(),
                Err(std::sync::TryLockError::WouldBlock) => return None,
            };
            self.activate_changes();
        }
        Some(self.subscribe_changes(options))
    }

    /// Caller holds the writer lock.
    fn activate_changes(&self) {
        let head = self
            .wal
            .as_ref()
            .map(|rec| rec.wal().next_lsn().raw().saturating_sub(1));
        self.changes.activate(head);
    }

    fn subscribe_changes(&self, options: ChangeFeedOptions) -> Result<ChangeFeed, LoraError> {
        match self.changes.subscribe(options)? {
            Resume::Ready(feed) => Ok(feed),
            Resume::NeedsHistory { feed, from, floor } => {
                let Some(rec) = &self.wal else {
                    return Err(LoraError::new(
                        crate::error::LoraErrorCode::ChangesTruncated,
                        format!(
                            "change feed cannot resume from LSN {from} because this in-memory database only retains batches after LSN {floor}; start a new feed without `fromLsn` and re-read current state"
                        ),
                    ));
                };
                let container = match &self.named_archive {
                    Some(archive) => archive.snapshot_bytes()?,
                    None => None,
                };
                let sources = HistorySources {
                    wal_dir: rec.wal().dir().to_path_buf(),
                    snapshots: self.snapshots.clone(),
                    container,
                };
                let history = HistoryReplay::plan(sources, from, floor)?;
                Ok(feed.with_history(history))
            }
        }
    }

    /// Number of recent batches kept in memory for same-process resume
    /// (default [`crate::DEFAULT_RETENTION`]).
    pub fn set_change_retention(&self, batches: usize) {
        self.changes.set_retention(batches);
    }

    /// LSN of the newest captured batch, or `None` while no feed has been
    /// opened on this database.
    pub fn changes_head(&self) -> Option<u64> {
        self.changes.head()
    }
}

impl<S> Database<S>
where
    S: GraphStorage + GraphStorageMut + Any + Clone + Send + Sync + 'static,
{
    /// Publish one committed write to the change feed. `pre` is the graph
    /// before the write (read only when it deleted something), `post` the
    /// graph after it. No-op for non-`InMemoryGraph` backends.
    pub(crate) fn publish_changes(
        &self,
        lsn: Option<Lsn>,
        events: &[MutationEvent],
        pre: Option<&S>,
        post: &S,
    ) {
        let pre = pre.and_then(|pre| (pre as &dyn Any).downcast_ref::<InMemoryGraph>());
        self.publish_changes_with(lsn, events, &PreImages::for_events(events, pre), post);
    }

    /// [`Self::publish_changes`] with deleted records already collected.
    pub(crate) fn publish_changes_with(
        &self,
        lsn: Option<Lsn>,
        events: &[MutationEvent],
        pre: &PreImages,
        post: &S,
    ) {
        let Some(post) = (post as &dyn Any).downcast_ref::<InMemoryGraph>() else {
            return;
        };
        publish_committed(&self.changes, lsn.map(Lsn::raw), events, pre, post);
    }

    /// Announce that the whole graph was replaced outside the WAL
    /// (snapshot restore, admin swap).
    pub(crate) fn publish_reset(&self) {
        if !self.changes.is_active() {
            return;
        }
        let next = self.wal.as_ref().map(|rec| rec.wal().next_lsn().raw());
        self.changes.publish_reset(next);
    }
}
