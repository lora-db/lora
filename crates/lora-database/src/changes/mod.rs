//! Committed-change feed.
//!
//! [`Database::changes`](crate::Database::changes) opens a [`ChangeFeed`]:
//! an ordered stream of [`ChangeBatch`]es, one per committed write
//! (auto-commit query, explicit transaction, streamed write, admin mutator,
//! `clear()`, snapshot restore). Rolled-back work never appears.
//!
//! # LSNs
//!
//! Every batch carries an `lsn`, a strictly increasing resume token.
//! WAL-backed databases use the LSN of the transaction's `TxCommit` record,
//! so tokens stay valid across restarts. In-memory databases use a
//! per-process commit counter.
//!
//! # Capture cost
//!
//! Capture is off until the first feed opens. From then on every write
//! builds its batch (net changes per entity plus property maps) under the
//! writer lock and keeps the last [`DEFAULT_RETENTION`] batches in memory
//! so feeds can resume from a recent LSN without touching disk.
//!
//! # Back-pressure
//!
//! Each feed has a bounded queue. Writers never wait for readers: when a
//! queue is full the feed ends with `LORA_CHANGES_LAGGED` after the
//! buffered batches are drained, and the consumer resumes from the last LSN
//! it processed.

mod build;
mod history;

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::time::Duration;

use lora_store::{InMemoryGraph, MutationEvent, NodeId, Properties, RelationshipId};

pub(crate) use build::{build_changes, PreImageSink, PreImages};
pub(crate) use history::{HistoryReplay, HistorySources};

use crate::error::{LoraError, LoraErrorCode};

/// Batches kept in memory for same-process resume.
pub const DEFAULT_RETENTION: usize = 1024;

/// Default per-feed queue capacity, in batches.
pub const DEFAULT_FEED_BUFFER: usize = 1024;

/// Net effect of one committed transaction on one entity.
///
/// Created and updated entities carry their state after the commit.
/// Deleted entities carry their last committed state. An entity created and
/// deleted inside the same transaction is not reported.
#[derive(Debug, Clone, PartialEq)]
pub enum Change {
    NodeCreated {
        id: NodeId,
        labels: Vec<String>,
        properties: Properties,
    },
    NodeUpdated {
        id: NodeId,
        labels: Vec<String>,
        properties: Properties,
        /// Keys the transaction set (last operation per key wins).
        set_keys: Vec<String>,
        /// Keys the transaction removed (last operation per key wins).
        removed_keys: Vec<String>,
        added_labels: Vec<String>,
        removed_labels: Vec<String>,
    },
    NodeDeleted {
        id: NodeId,
        labels: Vec<String>,
        properties: Properties,
    },
    RelationshipCreated {
        id: RelationshipId,
        rel_type: String,
        start: NodeId,
        end: NodeId,
        properties: Properties,
    },
    RelationshipUpdated {
        id: RelationshipId,
        rel_type: String,
        start: NodeId,
        end: NodeId,
        properties: Properties,
        set_keys: Vec<String>,
        removed_keys: Vec<String>,
    },
    RelationshipDeleted {
        id: RelationshipId,
        rel_type: String,
        start: NodeId,
        end: NodeId,
        properties: Properties,
    },
    /// The whole graph was replaced (`clear()` or a snapshot restore).
    /// Consumers should drop anything they derived from earlier batches.
    Reset,
}

/// Every change one committed write made, in the order it first touched
/// each entity.
#[derive(Debug, Clone, PartialEq)]
pub struct ChangeBatch {
    /// Strictly increasing resume token.
    pub lsn: u64,
    pub changes: Vec<Change>,
}

/// Options for [`Database::changes`](crate::Database::changes).
#[derive(Debug, Clone, Copy)]
pub struct ChangeFeedOptions {
    /// Resume after this LSN: the feed first yields every retained batch
    /// with a greater LSN, then live batches. `None` starts with the next
    /// commit.
    pub from_lsn: Option<u64>,
    /// Maximum number of undelivered live batches before the feed ends with
    /// `LORA_CHANGES_LAGGED`.
    pub buffer_size: usize,
}

impl Default for ChangeFeedOptions {
    fn default() -> Self {
        Self {
            from_lsn: None,
            buffer_size: DEFAULT_FEED_BUFFER,
        }
    }
}

/// Result of polling a [`ChangeFeed`].
#[derive(Debug, Clone)]
pub enum ChangePoll {
    /// The next batch in commit order.
    Batch(Arc<ChangeBatch>),
    /// Nothing buffered right now.
    Pending,
    /// The feed was closed (by the consumer or because the database shut
    /// down). No further batches follow.
    Closed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SubStatus {
    Open,
    Lagged,
    Closed,
}

type Waker = Arc<dyn Fn() + Send + Sync>;

struct SubState {
    queue: VecDeque<Arc<ChangeBatch>>,
    capacity: usize,
    status: SubStatus,
    waker: Option<Waker>,
}

pub(crate) struct Subscriber {
    state: Mutex<SubState>,
    ready: Condvar,
}

impl Subscriber {
    fn lock(&self) -> MutexGuard<'_, SubState> {
        self.state.lock().unwrap_or_else(|p| p.into_inner())
    }

    fn is_open(&self) -> bool {
        self.lock().status == SubStatus::Open
    }

    fn push(&self, batch: &Arc<ChangeBatch>) {
        let waker = {
            let mut state = self.lock();
            if state.status != SubStatus::Open {
                return;
            }
            if state.queue.len() >= state.capacity {
                state.status = SubStatus::Lagged;
                state.waker.clone()
            } else {
                state.queue.push_back(batch.clone());
                if state.queue.len() == 1 {
                    state.waker.clone()
                } else {
                    None
                }
            }
        };
        self.ready.notify_all();
        if let Some(waker) = waker {
            waker();
        }
    }

    fn close(&self) {
        let waker = {
            let mut state = self.lock();
            if state.status == SubStatus::Closed {
                return;
            }
            state.status = SubStatus::Closed;
            state.queue.clear();
            state.waker.take()
        };
        self.ready.notify_all();
        if let Some(waker) = waker {
            waker();
        }
    }
}

struct HubState {
    /// LSN of the newest published batch (or the capture start point).
    last_lsn: u64,
    /// Recent batches, oldest first.
    ring: VecDeque<Arc<ChangeBatch>>,
    /// Every published batch with an LSN above this one is in `ring`.
    ring_floor: u64,
    retention: usize,
    subscribers: Vec<Arc<Subscriber>>,
    closed: bool,
}

/// Per-database fan-out point. Write paths publish into it while holding
/// the writer lock, so batches arrive in commit order.
pub(crate) struct ChangeHub {
    active: AtomicBool,
    state: Mutex<HubState>,
}

impl Default for ChangeHub {
    fn default() -> Self {
        Self {
            active: AtomicBool::new(false),
            state: Mutex::new(HubState {
                last_lsn: 0,
                ring: VecDeque::new(),
                ring_floor: 0,
                retention: DEFAULT_RETENTION,
                subscribers: Vec::new(),
                closed: false,
            }),
        }
    }
}

impl ChangeHub {
    fn lock(&self) -> MutexGuard<'_, HubState> {
        self.state.lock().unwrap_or_else(|p| p.into_inner())
    }

    /// Whether writes should capture changes. Read under the writer lock.
    #[inline]
    pub(crate) fn is_active(&self) -> bool {
        self.active.load(Ordering::Acquire)
    }

    /// Turn capture on. The caller holds the writer lock and passes the
    /// LSN of the newest commit (WAL) or `None` for in-memory databases,
    /// so no commit can slip between "not captured" and "captured".
    pub(crate) fn activate(&self, head: Option<u64>) {
        let mut state = self.lock();
        if self.is_active() {
            return;
        }
        let head = head.unwrap_or(state.last_lsn).max(state.last_lsn);
        state.last_lsn = head;
        state.ring_floor = head;
        self.active.store(true, Ordering::Release);
    }

    pub(crate) fn set_retention(&self, batches: usize) {
        let mut state = self.lock();
        state.retention = batches;
        trim_ring(&mut state);
    }

    pub(crate) fn head(&self) -> Option<u64> {
        self.is_active().then(|| self.lock().last_lsn)
    }

    /// Publish one committed write. `lsn` is the WAL commit LSN; `None`
    /// allocates the next in-memory counter value. Empty batches (reads,
    /// catalog-only writes) are dropped.
    pub(crate) fn publish(&self, lsn: Option<u64>, changes: Vec<Change>) {
        if changes.is_empty() {
            return;
        }
        let mut state = self.lock();
        if state.closed {
            return;
        }
        let lsn = lsn.unwrap_or(state.last_lsn + 1).max(state.last_lsn + 1);
        state.last_lsn = lsn;
        let batch = Arc::new(ChangeBatch { lsn, changes });
        state.ring.push_back(batch.clone());
        trim_ring(&mut state);
        state.subscribers.retain(|sub| {
            sub.push(&batch);
            sub.is_open()
        });
    }

    /// Publish a `Reset` for a graph replacement that has no WAL record
    /// (snapshot restore). On a WAL-backed database the batch takes an LSN
    /// between the last commit and the next one; when two resets arrive
    /// without a commit between them the second is folded into the first.
    pub(crate) fn publish_reset(&self, wal_next_lsn: Option<u64>) {
        match wal_next_lsn {
            None => self.publish(None, vec![Change::Reset]),
            Some(next) => {
                let last = self.lock().last_lsn;
                let lsn = next.max(last + 1);
                // The next WAL commit record takes `next + 2`.
                if lsn <= next.saturating_add(1) {
                    self.publish(Some(lsn), vec![Change::Reset]);
                }
            }
        }
    }

    /// End every feed. Called when the database shuts down.
    pub(crate) fn close(&self) {
        let subscribers = {
            let mut state = self.lock();
            state.closed = true;
            std::mem::take(&mut state.subscribers)
        };
        for sub in subscribers {
            sub.close();
        }
    }
}

fn trim_ring(state: &mut HubState) {
    while state.ring.len() > state.retention {
        if let Some(evicted) = state.ring.pop_front() {
            state.ring_floor = evicted.lsn;
        }
    }
}

/// How a new feed catches up before it switches to live batches.
pub(crate) enum CatchUp {
    None,
    /// Rebuild `(from, floor]` from the WAL, then continue with `ring`.
    Wal(Box<HistoryReplay>),
}

/// Outcome of [`ChangeHub::subscribe`] before any history is planned.
pub(crate) enum Resume {
    /// Everything needed is in memory.
    Ready(ChangeFeed),
    /// `from` is older than the in-memory window; the caller must plan a
    /// WAL replay of `(from, floor]` and then call [`ChangeFeed::with_history`].
    NeedsHistory {
        feed: ChangeFeed,
        from: u64,
        floor: u64,
    },
}

impl ChangeHub {
    /// Register a feed. Runs under the hub lock only, so it never waits for
    /// an open transaction once capture is active.
    pub(crate) fn subscribe(
        self: &Arc<Self>,
        options: ChangeFeedOptions,
    ) -> Result<Resume, LoraError> {
        let mut state = self.lock();
        if state.closed {
            return Err(LoraError::new(
                LoraErrorCode::TransactionFailure,
                "database is closed",
            ));
        }
        let head = state.last_lsn;
        let sub = Arc::new(Subscriber {
            state: Mutex::new(SubState {
                queue: VecDeque::new(),
                capacity: options.buffer_size.max(1),
                status: SubStatus::Open,
                waker: None,
            }),
            ready: Condvar::new(),
        });
        let mut feed = ChangeFeed {
            sub: sub.clone(),
            backlog: VecDeque::new(),
            history: CatchUp::None,
            last_lsn: options.from_lsn,
            errored: false,
        };

        let resume = match options.from_lsn {
            None => Resume::Ready(feed),
            Some(from) if from > head => {
                return Err(LoraError::new(
                    LoraErrorCode::ChangesTruncated,
                    format!(
                        "change feed cannot resume from LSN {from} because the newest committed LSN is {head}; start a new feed without `fromLsn` and re-read current state"
                    ),
                ));
            }
            Some(from) => {
                feed.backlog = state
                    .ring
                    .iter()
                    .filter(|batch| batch.lsn > from)
                    .cloned()
                    .collect();
                if from >= state.ring_floor {
                    Resume::Ready(feed)
                } else {
                    Resume::NeedsHistory {
                        feed,
                        from,
                        floor: state.ring_floor,
                    }
                }
            }
        };
        state.subscribers.push(sub);
        Ok(resume)
    }
}

/// Ordered stream of committed [`ChangeBatch`]es from one database.
///
/// Poll it with [`Self::poll`] (non-blocking) or [`Self::next_timeout`].
/// Dropping the feed unsubscribes it.
pub struct ChangeFeed {
    sub: Arc<Subscriber>,
    /// Retained batches past `from_lsn`, delivered before live ones.
    backlog: VecDeque<Arc<ChangeBatch>>,
    history: CatchUp,
    last_lsn: Option<u64>,
    errored: bool,
}

/// Cloneable handle that closes a [`ChangeFeed`] from another thread.
#[derive(Clone)]
pub struct ChangeFeedCloser {
    sub: Arc<Subscriber>,
}

impl ChangeFeedCloser {
    pub fn close(&self) {
        self.sub.close();
    }
}

impl ChangeFeed {
    pub(crate) fn with_history(mut self, history: HistoryReplay) -> Self {
        self.history = CatchUp::Wal(Box::new(history));
        self
    }

    /// LSN of the last batch this feed delivered (or the `from_lsn` it was
    /// opened with). Pass it as `from_lsn` to resume.
    pub fn last_lsn(&self) -> Option<u64> {
        self.last_lsn
    }

    /// Handle for closing this feed from another thread.
    pub fn closer(&self) -> ChangeFeedCloser {
        ChangeFeedCloser {
            sub: self.sub.clone(),
        }
    }

    /// Close the feed. Later polls return [`ChangePoll::Closed`].
    pub fn close(&self) {
        self.sub.close();
    }

    /// Install a callback that fires when the feed goes from empty to
    /// non-empty, lags, or closes. It runs on the writing thread while the
    /// writer lock is held, so it must be quick and must not call back into
    /// the database.
    pub fn set_waker(&self, waker: impl Fn() + Send + Sync + 'static) {
        let notify_now = {
            let mut state = self.sub.lock();
            state.waker = Some(Arc::new(waker));
            (!state.queue.is_empty() || state.status != SubStatus::Open)
                .then(|| state.waker.clone())
                .flatten()
        };
        if let Some(waker) = notify_now {
            waker();
        }
    }

    fn deliver(&mut self, batch: Arc<ChangeBatch>) -> ChangePoll {
        self.last_lsn = Some(batch.lsn);
        ChangePoll::Batch(batch)
    }

    /// Next batch without blocking. History replay (resuming from an LSN
    /// older than the in-memory window) does its work inside this call.
    ///
    /// Fails with `LORA_CHANGES_LAGGED` once the feed has fallen behind and
    /// its buffered batches are drained; the feed is closed afterwards.
    pub fn poll(&mut self) -> Result<ChangePoll, LoraError> {
        if self.errored {
            return Ok(ChangePoll::Closed);
        }
        if let CatchUp::Wal(history) = &mut self.history {
            let sub = self.sub.clone();
            let stop = move || sub.lock().status == SubStatus::Closed;
            match history.next_batch(&stop) {
                Ok(Some(batch)) => {
                    // The ring backlog starts where the history ends.
                    let lsn = batch.lsn;
                    self.backlog.retain(|b| b.lsn > lsn);
                    return Ok(self.deliver(Arc::new(batch)));
                }
                Ok(None) => self.history = CatchUp::None,
                Err(err) => {
                    self.errored = true;
                    self.sub.close();
                    return Err(err);
                }
            }
        }
        if let Some(batch) = self.backlog.pop_front() {
            if self.last_lsn.is_none_or(|last| batch.lsn > last) {
                return Ok(self.deliver(batch));
            }
            return self.poll();
        }

        let mut state = self.sub.lock();
        if let Some(batch) = state.queue.pop_front() {
            drop(state);
            return Ok(self.deliver(batch));
        }
        match state.status {
            SubStatus::Open => Ok(ChangePoll::Pending),
            SubStatus::Closed => Ok(ChangePoll::Closed),
            SubStatus::Lagged => {
                state.status = SubStatus::Closed;
                drop(state);
                self.errored = true;
                Err(self.lagged_error())
            }
        }
    }

    fn lagged_error(&self) -> LoraError {
        let capacity = self.sub.lock().capacity;
        let hint = match self.last_lsn {
            Some(lsn) => format!("resume with `fromLsn` {lsn}"),
            None => "start a new feed and re-read current state".to_string(),
        };
        LoraError::new(
            LoraErrorCode::ChangesLagged,
            format!("change feed fell more than {capacity} batches behind the writers; {hint}"),
        )
    }

    /// Block until a batch arrives, the feed closes, or `timeout` passes
    /// (then [`ChangePoll::Pending`]).
    pub fn next_timeout(&mut self, timeout: Duration) -> Result<ChangePoll, LoraError> {
        let deadline = std::time::Instant::now() + timeout;
        loop {
            match self.poll()? {
                ChangePoll::Pending => {}
                other => return Ok(other),
            }
            let now = std::time::Instant::now();
            if now >= deadline {
                return Ok(ChangePoll::Pending);
            }
            let state = self.sub.lock();
            if state.queue.is_empty() && state.status == SubStatus::Open {
                let _ = self
                    .sub
                    .ready
                    .wait_timeout(state, deadline - now)
                    .unwrap_or_else(|p| p.into_inner());
            }
        }
    }
}

impl Drop for ChangeFeed {
    fn drop(&mut self) {
        self.sub.close();
    }
}

/// Build and publish the batch for one committed write against an
/// `InMemoryGraph`. `pre` holds the records the write deleted.
pub(crate) fn publish_committed(
    hub: &ChangeHub,
    lsn: Option<u64>,
    events: &[MutationEvent],
    pre: &PreImages,
    post: &InMemoryGraph,
) {
    if events.is_empty() {
        return;
    }
    hub.publish(lsn, build_changes(events, pre, post));
}

/// Recorder that buffers events for change capture on databases without a
/// WAL (the WAL recorder already buffers them otherwise).
#[derive(Default)]
pub(crate) struct CaptureRecorder {
    events: Mutex<Vec<MutationEvent>>,
}

impl CaptureRecorder {
    pub(crate) fn take(&self) -> Vec<MutationEvent> {
        std::mem::take(&mut *self.events.lock().unwrap_or_else(|p| p.into_inner()))
    }
}

impl lora_store::MutationRecorder for CaptureRecorder {
    fn record(&self, event: MutationEvent) {
        self.events
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .push(event);
    }
}
