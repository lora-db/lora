//! Stack growth for the recursive walks over queries and values.
//!
//! The parser, analyzer, compiler and executor walk queries recursively,
//! and [`crate::limits`] bounds how deep those walks go. The stack a walk
//! needs at the limits is more than some host threads have (a 128 KiB musl
//! thread, a 512 KiB macOS secondary thread), and a stack overflow aborts
//! the whole process. Every recursive step therefore runs through
//! [`guard`]: when less than [`RED_ZONE`] bytes of stack are left, the step
//! continues on a freshly allocated segment (the `stacker` crate), so a
//! query never overflows whatever stack the caller's thread has. With
//! enough stack left, a guard costs a thread-local read and a comparison.
//!
//! On wasm32 the stack cannot be switched; a guard runs its closure as is
//! and the limits are sized for the 1 MiB stack wasm builds get.

/// Stack a guarded step may use before it reaches the next guard: the
/// deepest stretch of frames between two guards, plus the value recursion
/// (at most [`crate::limits::MAX_VALUE_NESTING`] levels) a leaf step may
/// run. Unoptimised builds use several times more stack per frame.
pub const RED_ZONE: usize = if cfg!(debug_assertions) {
    4 << 20
} else {
    512 << 10
};

/// Run `f`, on a new stack segment if fewer than [`RED_ZONE`] bytes of the
/// current one are left.
#[inline(always)]
pub fn guard<R>(f: impl FnOnce() -> R) -> R {
    guard_with(RED_ZONE, f)
}

/// [`guard`] for a step that needs `red_zone` bytes before it reaches its
/// next guard, such as a call into a generated recursive-descent parser.
#[inline(always)]
pub fn guard_with<R>(red_zone: usize, f: impl FnOnce() -> R) -> R {
    #[cfg(not(target_family = "wasm"))]
    {
        native::guard_with(red_zone, f)
    }
    #[cfg(target_family = "wasm")]
    {
        let _ = red_zone;
        f()
    }
}

/// Levels a walk descends between two stack checks when it counts its
/// depth (see [`check_due`]).
pub const CHECK_INTERVAL: usize = 16;

/// Whether a recursive walk at `depth` (counting from 1) should pass
/// through a [`guard`]: every [`CHECK_INTERVAL`]th level. A walk over a
/// small plan or expression then never checks; its first levels run inside
/// the guard of the step that started it. For walks that run once per
/// query, where a check on every level would add up.
#[inline(always)]
pub fn check_due(depth: usize) -> bool {
    depth.is_multiple_of(CHECK_INTERVAL)
}

/// Set the size of the segments [`guard`] allocates, in bytes; `0`
/// restores the default (four red zones). Tests shrink it below the red
/// zone, so a stretch of recursion longer than that between two guards
/// overflows instead of running on spare segment.
#[doc(hidden)]
pub fn set_segment_size(bytes: usize) {
    #[cfg(not(target_family = "wasm"))]
    native::SEGMENT.store(bytes, std::sync::atomic::Ordering::Relaxed);
    #[cfg(target_family = "wasm")]
    let _ = bytes;
}

#[cfg(not(target_family = "wasm"))]
mod native {
    use std::cell::Cell;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::RED_ZONE;

    /// Segment size override; `0` is the default.
    pub(super) static SEGMENT: AtomicUsize = AtomicUsize::new(0);

    /// Not yet known: high enough that the first guard on a thread takes
    /// the slow path and looks the bound up. Red zones stay far below
    /// 1 GiB, so adding one cannot overflow.
    const UNKNOWN: usize = usize::MAX - (1 << 30);

    /// No bound can be found on this platform: no stack address is below
    /// it plus a red zone, so guards never grow.
    const UNBOUNDED: usize = 0;

    thread_local! {
        /// Lowest usable address of the stack this thread runs on now
        /// (stacks grow down on every platform `stacker` supports).
        static LIMIT: Cell<usize> = const { Cell::new(UNKNOWN) };
    }

    /// An address in the caller's frame: close enough to the stack
    /// pointer, as a red zone is far larger than a frame.
    #[inline(always)]
    fn stack_address() -> usize {
        let marker = 0u8;
        std::hint::black_box(&marker) as *const u8 as usize
    }

    #[inline(always)]
    pub(super) fn guard_with<R>(red_zone: usize, f: impl FnOnce() -> R) -> R {
        let limit = LIMIT.with(Cell::get);
        if stack_address() >= limit + red_zone {
            f()
        } else {
            low(red_zone, f)
        }
    }

    /// The stack is low, or this thread's bound is not known yet. Kept
    /// small and out of line: the callers are recursive hot paths.
    #[cold]
    #[inline(never)]
    fn low<R>(red_zone: usize, f: impl FnOnce() -> R) -> R {
        let mut f = Some(f);
        let mut result = None;
        low_dyn(red_zone, &mut || result = f.take().map(|f| f()));
        result.expect("a stack guard's step did not run")
    }

    /// [`low`] without the generics, so it is compiled once.
    fn low_dyn(red_zone: usize, step: &mut dyn FnMut()) {
        let Some(left) = stacker::remaining_stack() else {
            LIMIT.with(|l| l.set(UNBOUNDED));
            return step();
        };
        let limit = stack_address().saturating_sub(left);
        LIMIT.with(|l| l.set(limit));
        if left >= red_zone {
            return step();
        }
        let segment = match SEGMENT.load(Ordering::Relaxed) {
            0 => 4 * red_zone.max(RED_ZONE),
            // A test override: the extra a larger red zone asks for on top.
            bytes => bytes + red_zone.saturating_sub(RED_ZONE),
        };
        stacker::grow(segment, || {
            // `stacker` knows the new segment's bound; take it over, and
            // put the old one back when the step returns to the old stack.
            let segment_limit = stacker::remaining_stack()
                .map_or(UNBOUNDED, |left| stack_address().saturating_sub(left));
            let restore = Restore(limit);
            LIMIT.with(|l| l.set(segment_limit));
            step();
            drop(restore);
        })
    }

    /// Puts a thread's previous stack bound back, also when a panic
    /// unwinds out of a grown segment.
    struct Restore(usize);

    impl Drop for Restore {
        fn drop(&mut self) {
            LIMIT.with(|l| l.set(self.0));
        }
    }
}
