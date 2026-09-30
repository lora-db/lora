//! Helpers shared by the bindings' native row streams.
//!
//! - [`ShapeCache`]: whether a query streams as a write, per query text,
//!   so opening a stream does not pay an `explain()` every time.
//! - [`WriteStream`] (not on wasm32): a mutating stream kept on a thread of
//!   its own. A mutating stream holds the database's writer lock until it
//!   is exhausted or dropped, and that lock guard must be released on the
//!   thread that took it, while a host may pull and drop the stream from
//!   any thread. The stream therefore lives on an actor thread that opens
//!   it, pulls one row per request and drops it. Actor threads are pooled,
//!   and a request and its reply spin briefly before parking, so a stream
//!   drained in a loop does not pay a thread start per open or a park and
//!   wake-up per row.

use std::collections::HashMap;
use std::hash::{BuildHasherDefault, Hasher};
use std::sync::{Arc, Mutex, MutexGuard};

use lora_database::{Database, InMemoryGraph, LoraError};

/// Entries kept by a [`ShapeCache`] before it starts over.
const SHAPE_CACHE_CAPACITY: usize = 512;

/// How a query streams.
#[derive(Clone, Debug)]
pub struct StreamShape {
    /// The plan writes: the stream holds the writer lock while open.
    pub mutating: bool,
    /// The plan's result columns.
    pub columns: Arc<[String]>,
}

/// [`StreamShape`] per query text.
///
/// Classifying costs an `explain()` (a plan-cache lookup plus building a
/// plan tree), which dominated opening a small stream. Whether a plan
/// writes, and its columns, depend only on the query text, so the answer
/// is cached; a failure is not. Bounded: it starts over when full.
#[derive(Default)]
pub struct ShapeCache(Mutex<Shapes>);

type Shapes = HashMap<String, StreamShape, BuildHasherDefault<QueryHasher>>;

impl ShapeCache {
    pub fn classify(
        &self,
        db: &Database<InMemoryGraph>,
        query: &str,
    ) -> Result<StreamShape, LoraError> {
        if let Some(shape) = self.lock().get(query) {
            return Ok(shape.clone());
        }
        let plan = db.explain(query, None)?;
        let shape = StreamShape {
            mutating: plan.shape.is_mutating(),
            columns: plan.result_columns.into(),
        };
        let mut shapes = self.lock();
        if shapes.len() >= SHAPE_CACHE_CAPACITY {
            shapes.clear();
        }
        shapes.insert(query.to_owned(), shape.clone());
        Ok(shape)
    }

    fn lock(&self) -> MutexGuard<'_, Shapes> {
        self.0.lock().unwrap_or_else(|p| p.into_inner())
    }
}

/// A word-at-a-time multiplicative hash (FxHash). A lookup runs on every
/// stream open, where SipHash over the query text was a measurable share of
/// opening a small stream. Collisions only cost time, and the cache is
/// small and bounded.
#[derive(Default)]
struct QueryHasher(u64);

impl QueryHasher {
    fn add(&mut self, word: u64) {
        self.0 = (self.0.rotate_left(5) ^ word).wrapping_mul(0x517c_c1b7_2722_0a95);
    }
}

impl Hasher for QueryHasher {
    fn write(&mut self, bytes: &[u8]) {
        let mut words = bytes.chunks_exact(8);
        for word in &mut words {
            let mut buf = [0u8; 8];
            buf.copy_from_slice(word);
            self.add(u64::from_le_bytes(buf));
        }
        for &byte in words.remainder() {
            self.add(u64::from(byte));
        }
    }

    fn finish(&self) -> u64 {
        self.0
    }
}

#[cfg(not(target_arch = "wasm32"))]
pub use actor::{write_streams_active, OpeningStream, WriteStream};

#[cfg(not(target_arch = "wasm32"))]
mod actor {
    use std::collections::BTreeMap;
    use std::panic::{catch_unwind, AssertUnwindSafe};
    use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
    use std::sync::mpsc::{self, Receiver, RecvError, RecvTimeoutError, Sender, TryRecvError};
    use std::sync::{Arc, Mutex};
    use std::time::{Duration, Instant};

    use lora_database::{Database, InMemoryGraph, LoraError, LoraErrorCode, LoraValue, Row};

    type Db = Arc<Database<InMemoryGraph>>;
    type Pulled<T, E> = Result<Option<T>, E>;

    /// [`WriteStream`]s started and not yet done (their actor may hold or
    /// wait for the writer lock), process-wide.
    static ACTIVE: AtomicUsize = AtomicUsize::new(0);

    /// Whether any [`WriteStream`] may hold or wait for a writer lock.
    pub fn write_streams_active() -> bool {
        ACTIVE.load(Ordering::SeqCst) != 0
    }

    /// Counts a stream in [`ACTIVE`] while alive.
    struct Active;

    impl Active {
        fn enter() -> Self {
            ACTIVE.fetch_add(1, Ordering::SeqCst);
            Self
        }
    }

    impl Drop for Active {
        fn drop(&mut self) {
            ACTIVE.fetch_sub(1, Ordering::SeqCst);
        }
    }

    /// A mutating stream on its actor thread. Pull it and drop it on any
    /// thread; dropping an unfinished one rolls it back and releases the
    /// writer lock before `drop` returns.
    pub struct WriteStream<T, E> {
        columns: Vec<String>,
        requests: Option<Sender<()>>,
        replies: Receiver<Pulled<T, E>>,
        /// Disconnects once the actor has dropped the stream.
        done: Receiver<()>,
        ended: bool,
    }

    impl<T, E> WriteStream<T, E>
    where
        T: Send + 'static,
        E: From<LoraError> + Send + 'static,
    {
        /// Open `query` on an actor thread and wait until it is open, i.e.
        /// holds the writer lock, or failed to open. Each row is mapped
        /// with `map` on the actor thread.
        pub fn open<F>(
            db: Db,
            query: String,
            params: BTreeMap<String, LoraValue>,
            map: F,
        ) -> Result<Self, E>
        where
            F: FnMut(Row) -> Result<T, E> + Send + 'static,
        {
            Self::start(db, query, params, map)?.wait()
        }

        /// Start opening `query` on an actor thread and return at once; see
        /// [`OpeningStream`].
        pub fn start<F>(
            db: Db,
            query: String,
            params: BTreeMap<String, LoraValue>,
            mut map: F,
        ) -> Result<OpeningStream<T, E>, E>
        where
            F: FnMut(Row) -> Result<T, E> + Send + 'static,
        {
            let (requests, inbox) = mpsc::channel::<()>();
            let (reply, replies) = mpsc::channel::<Pulled<T, E>>();
            let (opened_tx, opened) = mpsc::sync_channel::<Result<Vec<String>, E>>(1);
            let (done_tx, done) = mpsc::channel::<()>();
            let active = Active::enter();
            let job = move || {
                // Dropped last, once the stream (and its lock) is gone:
                // `active` first, so a handle's `drop` sees it gone.
                let _done = done_tx;
                let _active = active;
                // SAFETY: `db` outlives `stream` (declared first, dropped
                // last) and the stream never leaves this thread.
                let db = db;
                let mut stream = match unsafe { db.stream_with_params_owned(&query, params) } {
                    Ok(stream) => stream,
                    Err(e) => {
                        let _ = opened_tx.send(Err(E::from(e)));
                        return;
                    }
                };
                if opened_tx.send(Ok(stream.columns().to_vec())).is_err() {
                    // Nobody waits for it any more: roll back.
                    return;
                }
                while recv(&inbox).is_ok() {
                    let pulled = match stream.next_row() {
                        Ok(Some(row)) => map(row).map(Some),
                        Ok(None) => Ok(None),
                        Err(e) => Err(E::from(LoraError::from_anyhow(e))),
                    };
                    if matches!(pulled, Ok(Some(_))) {
                        if reply.send(pulled).is_err() {
                            return;
                        }
                    } else {
                        // End or error: finish the stream (commit or roll
                        // back) and release the writer lock before the
                        // caller hears of it.
                        drop(stream);
                        let _ = reply.send(pulled);
                        return;
                    }
                }
                // The handle was dropped: `stream` drops here, rolling back
                // if it was not exhausted.
            };
            run_pooled(Box::new(job))
                .map_err(|e| E::from(internal(format!("could not start stream thread: {e}"))))?;
            Ok(OpeningStream {
                parts: Some((requests, replies, done)),
                opened,
            })
        }

        pub fn columns(&self) -> &[String] {
            &self.columns
        }

        /// Pull the next row: `None` at the end (the writes are then
        /// committed). An error rolls the writes back. Both end the
        /// stream; later pulls report the end.
        pub fn next_row(&mut self) -> Pulled<T, E> {
            if self.ended {
                return Ok(None);
            }
            let closed = || E::from(internal("query stream thread exited".into()));
            let sent = self.requests.as_ref().map(|requests| requests.send(()));
            if !matches!(sent, Some(Ok(()))) {
                self.ended = true;
                return Err(closed());
            }
            let pulled = recv(&self.replies).map_err(|_| closed()).and_then(|p| p);
            if !matches!(pulled, Ok(Some(_))) {
                self.ended = true;
            }
            pulled
        }
    }

    /// A [`WriteStream`] whose actor may still wait for the writer lock.
    /// Dropping it does not wait: the actor rolls the stream back once it
    /// has opened it.
    pub struct OpeningStream<T, E> {
        parts: Option<Parts<T, E>>,
        opened: Receiver<Result<Vec<String>, E>>,
    }

    type Parts<T, E> = (Sender<()>, Receiver<Pulled<T, E>>, Receiver<()>);

    impl<T, E> OpeningStream<T, E>
    where
        E: From<LoraError>,
    {
        /// Wait until the stream is open or failed to open.
        pub fn wait(self) -> Result<WriteStream<T, E>, E> {
            let opened = recv(&self.opened);
            self.finish(opened)
        }

        /// The stream, if it opens (or fails to) within a brief spin, as
        /// it does at once when the writer lock is free; otherwise `self`
        /// back, still opening.
        pub fn try_open(self) -> Result<Result<WriteStream<T, E>, E>, Self> {
            match spin(&self.opened) {
                Some(Ok(opened)) => Ok(self.finish(Ok(opened))),
                Some(Err(_)) => Ok(self.finish(Err(RecvError))),
                None => Err(self),
            }
        }

        fn finish(
            mut self,
            opened: Result<Result<Vec<String>, E>, RecvError>,
        ) -> Result<WriteStream<T, E>, E> {
            let columns =
                opened.map_err(|_| E::from(internal("stream thread exited".into())))??;
            let (requests, replies, done) = self
                .parts
                .take()
                .ok_or_else(|| E::from(internal("stream already opened".into())))?;
            Ok(WriteStream {
                columns,
                requests: Some(requests),
                replies,
                done,
                ended: false,
            })
        }
    }

    impl<T, E> Drop for WriteStream<T, E> {
        /// Stop the actor and wait for it to drop the stream (rolling back
        /// an unfinished one). The actor is idle between pulls, so this is
        /// prompt.
        fn drop(&mut self) {
            self.requests.take();
            let _ = recv(&self.done);
        }
    }

    fn internal(message: String) -> LoraError {
        LoraError::new(LoraErrorCode::Internal, message)
    }

    /// How long a waiting side spins before it parks. A stream drained in
    /// a loop answers well within it; a slow consumer costs at most this
    /// much CPU per row on the actor.
    const SPIN: Duration = Duration::from_micros(20);

    /// `rx.recv()`, spinning briefly first: a park and wake-up costs
    /// several microseconds, many times a row's pull.
    fn recv<M>(rx: &Receiver<M>) -> Result<M, RecvError> {
        match spin(rx) {
            Some(Ok(m)) => Ok(m),
            Some(Err(_)) => Err(RecvError),
            None => rx.recv(),
        }
    }

    /// Poll `rx` for up to [`SPIN`]: `None` if nothing arrived meanwhile.
    fn spin<M>(rx: &Receiver<M>) -> Option<Result<M, TryRecvError>> {
        let poll = || match rx.try_recv() {
            Err(TryRecvError::Empty) => None,
            other => Some(other),
        };
        for _ in 0..64 {
            if let Some(got) = poll() {
                return Some(got);
            }
            std::hint::spin_loop();
        }
        let start = Instant::now();
        while start.elapsed() < SPIN {
            if let Some(got) = poll() {
                return Some(got);
            }
            std::thread::yield_now();
        }
        None
    }

    type Job = Box<dyn FnOnce() + Send>;

    /// Idle actor threads kept for reuse.
    const MAX_IDLE: usize = 8;
    /// An idle actor thread exits after this long without work.
    const IDLE_TIMEOUT: Duration = Duration::from_secs(10);

    static IDLE: Mutex<Vec<(u64, Sender<Job>)>> = Mutex::new(Vec::new());
    static NEXT_ID: AtomicU64 = AtomicU64::new(0);

    fn idle() -> std::sync::MutexGuard<'static, Vec<(u64, Sender<Job>)>> {
        IDLE.lock().unwrap_or_else(|p| p.into_inner())
    }

    /// Run `job` on an idle actor thread, or a new one. A job owns its
    /// thread until it returns; it may wait (for the writer lock) as long
    /// as it likes.
    fn run_pooled(mut job: Job) -> std::io::Result<()> {
        loop {
            let Some((_, worker)) = idle().pop() else {
                break;
            };
            match worker.send(job) {
                Ok(()) => return Ok(()),
                // That thread has exited; try the next.
                Err(mpsc::SendError(back)) => job = back,
            }
        }
        std::thread::Builder::new()
            .name("lora-stream".into())
            .spawn(move || work(job))
            .map(|_| ())
    }

    fn work(mut job: Job) {
        let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
        loop {
            // A panicking job has already dropped what it owned; the
            // thread survives it.
            let _ = catch_unwind(AssertUnwindSafe(job));
            let (tx, rx) = mpsc::channel::<Job>();
            {
                let mut idle = idle();
                if idle.len() >= MAX_IDLE {
                    return;
                }
                idle.push((id, tx));
            }
            // Streams often open back to back: catch the next one before
            // parking.
            let next = match spin(&rx) {
                Some(Ok(job)) => Ok(job),
                Some(Err(_)) => Err(RecvTimeoutError::Disconnected),
                None => rx.recv_timeout(IDLE_TIMEOUT),
            };
            job = match next {
                Ok(job) => job,
                Err(RecvTimeoutError::Disconnected) => return,
                Err(RecvTimeoutError::Timeout) => {
                    let mut idle = idle();
                    if let Some(at) = idle.iter().position(|(i, _)| *i == id) {
                        // Still listed, so nobody took this thread: no job
                        // is on its way.
                        idle.swap_remove(at);
                        return;
                    }
                    drop(idle);
                    // Taken just now: its job is on the way.
                    match rx.recv() {
                        Ok(job) => job,
                        Err(_) => return,
                    }
                }
            };
        }
    }
}
