//! Large buffers that live only while one step of an open or a replay
//! runs.
//!
//! macOS's malloc keeps freed large blocks resident for reuse and holds
//! them until the process exits, so a step that frees one per frame (the
//! snapshot reader's inflated frames, replay's WAL frames) leaves an open
//! holding many of them on top of the graph: 0.34.0's framed snapshot
//! read added about 60 B of resident memory per node of a 200k-node
//! graph this way (G-64). On macOS a [`ScratchBytes`] of
//! [`ScratchBytes::MAPPED_MIN`] bytes or more gets pages of its own,
//! mapped for it and unmapped when it is dropped. Elsewhere, and below
//! that size, it is a `Vec`: glibc maps blocks this large itself and
//! returns them when they are freed.

/// Zeroed bytes of a fixed length, returned to the operating system when
/// dropped if they are large (see the module docs).
pub struct ScratchBytes {
    #[cfg(target_os = "macos")]
    mapped: Option<mapped::Pages>,
    heap: Vec<u8>,
}

impl ScratchBytes {
    /// Smallest buffer given pages of its own on macOS.
    pub const MAPPED_MIN: usize = 128 << 10;

    /// `len` zeroed bytes, or `None` when they cannot be allocated. Pages
    /// are mapped lazily, so a `len` read from a file costs address space,
    /// not memory, until the bytes are written.
    pub fn zeroed(len: usize) -> Option<Self> {
        #[cfg(target_os = "macos")]
        if len >= Self::MAPPED_MIN {
            // Fresh anonymous pages read as zero.
            return mapped::Pages::new(len).map(|pages| ScratchBytes {
                mapped: Some(pages),
                heap: Vec::new(),
            });
        }
        let mut heap = Vec::new();
        heap.try_reserve_exact(len).ok()?;
        heap.resize(len, 0);
        Some(ScratchBytes {
            #[cfg(target_os = "macos")]
            mapped: None,
            heap,
        })
    }

    /// `bytes` as they are, on the heap.
    pub fn from_vec(bytes: Vec<u8>) -> Self {
        ScratchBytes {
            #[cfg(target_os = "macos")]
            mapped: None,
            heap: bytes,
        }
    }
}

impl std::ops::Deref for ScratchBytes {
    type Target = [u8];

    #[inline]
    fn deref(&self) -> &[u8] {
        #[cfg(target_os = "macos")]
        if let Some(pages) = &self.mapped {
            return pages.as_slice();
        }
        &self.heap
    }
}

impl std::ops::DerefMut for ScratchBytes {
    #[inline]
    fn deref_mut(&mut self) -> &mut [u8] {
        #[cfg(target_os = "macos")]
        if let Some(pages) = &mut self.mapped {
            return pages.as_mut_slice();
        }
        &mut self.heap
    }
}

impl std::fmt::Debug for ScratchBytes {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "ScratchBytes({} bytes)", self.len())
    }
}

/// A vector of plain data with a capacity known up front, kept as
/// [`ScratchBytes`] keeps its bytes: on macOS, in pages of its own when it
/// takes [`ScratchBytes::MAPPED_MIN`] bytes or more. A push past the
/// capacity moves it to the heap, so a short count costs a copy, never an
/// error. `T: Copy`, so nothing in it needs dropping.
pub struct ScratchVec<T: Copy> {
    #[cfg(target_os = "macos")]
    mapped: Option<(mapped::Pages, usize)>,
    heap: Vec<T>,
}

impl<T: Copy> ScratchVec<T> {
    pub fn with_capacity(cap: usize) -> Self {
        #[cfg(target_os = "macos")]
        {
            let size = std::mem::size_of::<T>();
            // Mapped pages are aligned to 4 KiB or more.
            let fits = size > 0 && std::mem::align_of::<T>() <= 4096;
            if let Some(bytes) = cap
                .checked_mul(size)
                .filter(|&b| fits && b >= ScratchBytes::MAPPED_MIN)
            {
                if let Some(pages) = mapped::Pages::new(bytes) {
                    return ScratchVec {
                        mapped: Some((pages, 0)),
                        heap: Vec::new(),
                    };
                }
            }
        }
        ScratchVec {
            #[cfg(target_os = "macos")]
            mapped: None,
            heap: Vec::with_capacity(cap),
        }
    }

    pub fn push(&mut self, value: T) {
        #[cfg(target_os = "macos")]
        if let Some((pages, len)) = &mut self.mapped {
            let cap = pages.len() / std::mem::size_of::<T>();
            if *len < cap {
                // SAFETY: slot `len < cap` lies in the mapping, which is
                // aligned for `T` (checked in `with_capacity`).
                unsafe { pages.as_ptr().cast::<T>().add(*len).write(value) };
                *len += 1;
                return;
            }
            let mut heap = Vec::with_capacity(cap.saturating_mul(2).max(1));
            heap.extend_from_slice(self.as_slice());
            self.mapped = None;
            self.heap = heap;
        }
        self.heap.push(value);
    }

    pub fn len(&self) -> usize {
        self.as_slice().len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn as_slice(&self) -> &[T] {
        #[cfg(target_os = "macos")]
        if let Some((pages, len)) = &self.mapped {
            // SAFETY: the first `len` slots were written by `push`.
            return unsafe { std::slice::from_raw_parts(pages.as_ptr().cast::<T>(), *len) };
        }
        &self.heap
    }

    pub fn as_mut_slice(&mut self) -> &mut [T] {
        #[cfg(target_os = "macos")]
        if let Some((pages, len)) = &mut self.mapped {
            // SAFETY: as above; `&mut self` makes the access exclusive.
            return unsafe { std::slice::from_raw_parts_mut(pages.as_ptr().cast::<T>(), *len) };
        }
        &mut self.heap
    }

    /// Copy every element of `other` onto the end.
    pub fn extend_from(&mut self, other: &ScratchVec<T>) {
        for &value in other.as_slice() {
            self.push(value);
        }
    }
}

// SAFETY: a `ScratchVec<T>` owns its elements as a `Vec<T>` does.
unsafe impl<T: Copy + Send> Send for ScratchVec<T> {}
unsafe impl<T: Copy + Sync> Sync for ScratchVec<T> {}

impl<T: Copy> Default for ScratchVec<T> {
    fn default() -> Self {
        Self::with_capacity(0)
    }
}

#[cfg(target_os = "macos")]
mod mapped {
    use std::ffi::{c_int, c_void};
    use std::ptr::NonNull;

    const PROT_READ: c_int = 0x1;
    const PROT_WRITE: c_int = 0x2;
    const MAP_PRIVATE: c_int = 0x2;
    const MAP_ANON: c_int = 0x1000;
    const MAP_FAILED: *mut c_void = !0 as *mut c_void;

    unsafe extern "C" {
        fn mmap(
            addr: *mut c_void,
            len: usize,
            prot: c_int,
            flags: c_int,
            fd: c_int,
            offset: i64,
        ) -> *mut c_void;
        fn munmap(addr: *mut c_void, len: usize) -> c_int;
    }

    /// Anonymous private pages holding `len` bytes, unmapped on drop.
    pub(super) struct Pages {
        ptr: NonNull<u8>,
        len: usize,
    }

    // SAFETY: `Pages` owns its mapping exclusively, like a `Vec<u8>`.
    unsafe impl Send for Pages {}
    unsafe impl Sync for Pages {}

    impl Pages {
        pub(super) fn new(len: usize) -> Option<Self> {
            // SAFETY: a fresh anonymous private mapping; no existing memory
            // is touched. Checked against `MAP_FAILED` before use.
            let ptr = unsafe {
                mmap(
                    std::ptr::null_mut(),
                    len,
                    PROT_READ | PROT_WRITE,
                    MAP_PRIVATE | MAP_ANON,
                    -1,
                    0,
                )
            };
            if ptr == MAP_FAILED {
                return None;
            }
            Some(Pages {
                ptr: NonNull::new(ptr.cast())?,
                len,
            })
        }

        pub(super) fn as_ptr(&self) -> *mut u8 {
            self.ptr.as_ptr()
        }

        pub(super) fn len(&self) -> usize {
            self.len
        }

        pub(super) fn as_slice(&self) -> &[u8] {
            // SAFETY: `len` mapped, readable bytes owned by `self`.
            unsafe { std::slice::from_raw_parts(self.ptr.as_ptr(), self.len) }
        }

        pub(super) fn as_mut_slice(&mut self) -> &mut [u8] {
            // SAFETY: as above, and `&mut self` makes the access exclusive.
            unsafe { std::slice::from_raw_parts_mut(self.ptr.as_ptr(), self.len) }
        }
    }

    impl Drop for Pages {
        fn drop(&mut self) {
            // SAFETY: the mapping `new` made, unmapped once.
            unsafe {
                munmap(self.ptr.as_ptr().cast(), self.len);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{ScratchBytes, ScratchVec};

    #[test]
    fn a_scratch_vec_keeps_its_elements_past_its_capacity() {
        // Large enough to be mapped on macOS, then past its capacity.
        let cap = ScratchBytes::MAPPED_MIN / std::mem::size_of::<(u64, u32)>() + 3;
        let mut v = ScratchVec::with_capacity(cap);
        for i in 0..cap as u64 {
            v.push((cap as u64 - i, i as u32));
        }
        v.as_mut_slice().sort_unstable();
        assert!(v.as_slice().windows(2).all(|w| w[0] < w[1]));
        v.push((0, 0));
        assert_eq!(v.len(), cap + 1);
        assert_eq!(v.as_slice()[0].0, 1);
        assert_eq!(v.as_slice()[cap], (0, 0));

        let mut a = ScratchVec::with_capacity(2);
        a.push(1);
        let mut b = ScratchVec::with_capacity(cap);
        b.push(2);
        b.push(3);
        a.extend_from(&b);
        assert_eq!(a.as_slice(), &[1, 2, 3]);
        assert!(ScratchVec::<u8>::default().is_empty());
    }

    #[test]
    fn small_and_large_buffers_are_zeroed_and_writable() {
        for len in [
            0,
            1,
            ScratchBytes::MAPPED_MIN - 1,
            ScratchBytes::MAPPED_MIN,
            3 << 20,
        ] {
            let mut buf = ScratchBytes::zeroed(len).expect("allocates");
            assert_eq!(buf.len(), len);
            assert!(buf.iter().all(|&b| b == 0));
            if len > 0 {
                buf[len - 1] = 7;
                assert_eq!(buf[len - 1], 7);
            }
        }
    }
}
