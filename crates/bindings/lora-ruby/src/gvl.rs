//! Global VM Lock release primitive.

use std::ffi::c_void;
use std::mem::MaybeUninit;

/// Run `f` with Ruby's Global VM Lock released.
///
/// Semantics match `rb_thread_call_without_gvl` — other Ruby threads can
/// progress while `f` runs. The closure MUST NOT touch Ruby state (no
/// `Value`s, no allocations into the Ruby heap), which we arrange by
/// keeping all such work on the calling thread. Everything inside
/// `database_execute`'s closure is pure Rust on pre-extracted data, so
/// this is sound.
pub(crate) fn without_gvl<F, R>(f: F) -> R
where
    F: FnOnce() -> R,
    F: Send,
    R: Send,
{
    struct Data<F, R> {
        func: Option<F>,
        result: MaybeUninit<R>,
    }

    unsafe extern "C" fn trampoline<F, R>(data: *mut c_void) -> *mut c_void
    where
        F: FnOnce() -> R,
    {
        let data = &mut *(data as *mut Data<F, R>);
        let f = data
            .func
            .take()
            .expect("without_gvl: closure already taken");
        data.result.write(f());
        std::ptr::null_mut()
    }

    let mut data = Data::<F, R> {
        func: Some(f),
        result: MaybeUninit::uninit(),
    };

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
        data.result.assume_init()
    }
}
