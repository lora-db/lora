//! Hand memory the process freed back to the operating system.
//!
//! Opening, checkpointing and closing a large named database free as much
//! as they allocate, much of it on whichever thread ran them. glibc's
//! malloc keeps what a thread's arena frees for that arena, and returns it
//! to the OS only from the top of a heap, so memory freed between live
//! allocations stays resident: a process that closed a 2 GB graph and
//! opened it again held both. jemalloc returns freed pages after a decay
//! period (10 s by default), so a reopen within it holds both as well.
//!
//! [`release_free_memory`] runs after those steps. On Linux with glibc it
//! calls `malloc_trim(0)`, which returns every free page of every arena,
//! and, when jemalloc was preloaded (`LD_PRELOAD`), purges jemalloc's
//! arenas through its `mallctl`. Elsewhere it does nothing: macOS's
//! allocator and Windows' heap return freed pages by themselves.

/// See the module docs. Takes milliseconds on a heap of a few gigabytes.
#[cfg(all(target_os = "linux", target_env = "gnu"))]
pub(crate) fn release_free_memory() {
    use std::ffi::{c_char, c_int, c_void};

    type Mallctl = unsafe extern "C" fn(
        name: *const c_char,
        oldp: *mut c_void,
        oldlenp: *mut usize,
        newp: *mut c_void,
        newlen: usize,
    ) -> c_int;

    unsafe extern "C" {
        fn malloc_trim(pad: usize) -> c_int;
        fn dlsym(handle: *mut c_void, symbol: *const c_char) -> *mut c_void;
    }

    // SAFETY: `dlsym` with RTLD_DEFAULT (a null handle on glibc) only looks
    // a symbol up. A `mallctl` it finds is jemalloc's, whose signature is
    // `Mallctl`; "arena.<MALLCTL_ARENAS_ALL>.purge" takes no arguments.
    // `malloc_trim` is glibc's and takes a byte count.
    unsafe {
        let mallctl = dlsym(std::ptr::null_mut(), c"mallctl".as_ptr());
        if !mallctl.is_null() {
            let mallctl: Mallctl = std::mem::transmute::<*mut c_void, Mallctl>(mallctl);
            mallctl(
                c"arena.4096.purge".as_ptr(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                0,
            );
        }
        malloc_trim(0);
    }
}

#[cfg(not(all(target_os = "linux", target_env = "gnu")))]
pub(crate) fn release_free_memory() {}
