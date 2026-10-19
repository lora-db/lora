//! LEB128 varints: seven bits per byte, low bits first, the high bit set
//! on every byte but the last. Used by the record encoding
//! ([`crate::encoded`]) and the length prefixes of the in-memory record
//! store.
//!
//! The bytes read are ones this crate wrote, so a malformed varint is a
//! bug, reported by the slice index.
//!
//! Every function is `#[inline(always)]`: they are a few instructions
//! each and sit in the record decoder's inner loops.

/// Bytes `value` takes as a varint.
#[inline(always)]
pub(crate) fn len(value: u64) -> usize {
    (64 - (value | 1).leading_zeros() as usize).div_ceil(7)
}

/// Append `value` to `out`.
#[inline(always)]
pub(crate) fn push(out: &mut Vec<u8>, mut value: u64) {
    while value >= 0x80 {
        out.push(value as u8 | 0x80);
        value >>= 7;
    }
    out.push(value as u8);
}

/// Write `value` into `out` at `*pos`, advancing it. `out` must have
/// [`len`]`(value)` bytes there.
#[inline(always)]
pub(crate) fn write(out: &mut [u8], pos: &mut usize, mut value: u64) {
    while value >= 0x80 {
        out[*pos] = value as u8 | 0x80;
        *pos += 1;
        value >>= 7;
    }
    out[*pos] = value as u8;
    *pos += 1;
}

/// Read a varint at `*pos`, advancing it.
#[inline(always)]
pub(crate) fn read(bytes: &[u8], pos: &mut usize) -> u64 {
    let first = bytes[*pos];
    *pos += 1;
    if first < 0x80 {
        return u64::from(first);
    }
    let mut value = u64::from(first & 0x7f);
    let mut shift = 7;
    loop {
        let byte = bytes[*pos];
        *pos += 1;
        value |= u64::from(byte & 0x7f) << shift;
        if byte < 0x80 {
            return value;
        }
        shift += 7;
    }
}

/// [`read`] for a length prefix, as a `usize`. A plain loop: where the
/// record store reads a record's length, it measured faster than
/// `read`'s one-byte fast path (raw `with_node`, 2M nodes: 47 against
/// 58 ns).
#[inline(always)]
pub(crate) fn read_len(bytes: &[u8], pos: &mut usize) -> usize {
    let mut value = 0usize;
    let mut shift = 0;
    loop {
        let byte = bytes[*pos];
        *pos += 1;
        value |= ((byte & 0x7f) as usize) << shift;
        if byte < 0x80 {
            return value;
        }
        shift += 7;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn varints_round_trip_through_both_writers() {
        for value in [0u64, 1, 127, 128, 16_383, 16_384, u32::MAX as u64, u64::MAX] {
            let mut pushed = Vec::new();
            push(&mut pushed, value);
            assert_eq!(pushed.len(), len(value), "{value}");
            let mut written = vec![0; len(value) + 2];
            let mut end = 1;
            write(&mut written, &mut end, value);
            assert_eq!(&written[1..end], &pushed[..]);
            let mut pos = 0;
            assert_eq!(read(&pushed, &mut pos), value);
            assert_eq!(pos, pushed.len());
            if let Ok(value) = usize::try_from(value) {
                let mut pos = 0;
                assert_eq!(read_len(&pushed, &mut pos), value);
                assert_eq!(pos, pushed.len());
            }
        }
    }
}
