//! Global VM Lock release primitive.

use std::ffi::c_void;
use std::fmt;
use std::mem::MaybeUninit;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::atomic::{AtomicUsize, Ordering};

/// Calls running with the GVL released right now, process-wide.
static RELEASED: AtomicUsize = AtomicUsize::new(0);

/// Whether no thread runs a GVL-free call. Called with the GVL held, this
/// means no other Ruby thread can hold or take the engine's writer lock
/// before the caller does: taking it needs either the GVL or a GVL-free
/// call (the binding has no interactive transactions or streams, and it
/// is not Ractor-safe, so there is one GVL). Background threads of a
/// persistent database are not covered.
pub(crate) fn nothing_released() -> bool {
    RELEASED.load(Ordering::SeqCst) == 0
}

/// Counts a call in [`RELEASED`] while alive.
struct Released;

impl Released {
    fn enter() -> Self {
        RELEASED.fetch_add(1, Ordering::SeqCst);
        Self
    }
}

impl Drop for Released {
    fn drop(&mut self) {
        RELEASED.fetch_sub(1, Ordering::SeqCst);
    }
}

pub(crate) struct GvlPanic {
    payload: Box<dyn std::any::Any + Send>,
}

impl GvlPanic {
    fn new(payload: Box<dyn std::any::Any + Send>) -> Self {
        Self { payload }
    }

    fn message(&self) -> &str {
        if let Some(s) = self.payload.downcast_ref::<&'static str>() {
            s
        } else if let Some(s) = self.payload.downcast_ref::<String>() {
            s.as_str()
        } else {
            "non-string panic payload"
        }
    }
}

impl fmt::Display for GvlPanic {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "engine panicked while Ruby GVL was released: {}",
            self.message()
        )
    }
}

/// Run `f` with Ruby's Global VM Lock released.
///
/// Every engine call that can wait for the writer lock goes through here.
/// The binding has no interactive transactions: a writer takes the lock
/// and releases it inside one call, entirely GVL-free, so the holder never
/// needs the GVL to finish and a waiter always sees it released. Waiting
/// with the GVL held would freeze every Ruby thread for the wait (and, with
/// a lock held across Ruby calls, deadlock).
///
/// Semantics match `rb_thread_call_without_gvl` — other Ruby threads can
/// progress while `f` runs. The closure MUST NOT touch Ruby state (no
/// `Value`s, no allocations into the Ruby heap), which we arrange by
/// keeping all such work on the calling thread. Everything inside
/// `database_execute`'s closure is pure Rust on pre-extracted data, so
/// this is sound.
pub(crate) fn without_gvl<F, R>(f: F) -> Result<R, GvlPanic>
where
    F: FnOnce() -> R,
    F: Send,
    R: Send,
{
    struct Data<F, R> {
        func: Option<F>,
        result: MaybeUninit<std::thread::Result<R>>,
    }

    unsafe extern "C" fn trampoline<F, R>(data: *mut c_void) -> *mut c_void
    where
        F: FnOnce() -> R,
    {
        let data = &mut *(data as *mut Data<F, R>);
        let result = match data.func.take() {
            Some(f) => catch_unwind(AssertUnwindSafe(f)),
            None => {
                Err(Box::new("without_gvl: closure already taken") as Box<dyn std::any::Any + Send>)
            }
        };
        data.result.write(result);
        std::ptr::null_mut()
    }

    let mut data = Data::<F, R> {
        func: Some(f),
        result: MaybeUninit::uninit(),
    };
    let _released = Released::enter();

    unsafe {
        rb_sys::rb_thread_call_without_gvl(
            Some(trampoline::<F, R>),
            &mut data as *mut _ as *mut c_void,
            // No unblock function — the engine doesn't implement
            // cooperative cancellation, and a forced longjmp out of a
            // mutex-holding section would be worse than waiting.
            None,
            std::ptr::null_mut(),
        );
        data.result.assume_init().map_err(GvlPanic::new)
    }
}
