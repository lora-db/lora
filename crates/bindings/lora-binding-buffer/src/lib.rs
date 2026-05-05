//! Compact binary encoding for query results.
//!
//! Earlier versions of the binding built one JS object per row and one
//! `set_property` per cell on the JS main thread. Each crossing into v8
//! costs ~150 ns; for a 10 000-row scan that's tens of thousands of
//! syscalls and dominates the wall-clock time.
//!
//! Instead we encode the whole result into a single `Vec<u8>` on the
//! libuv worker, hand the buffer to JS as a `Buffer` in one napi call,
//! and let V8 decode it in JIT'd JavaScript. V8 walks contiguous bytes
//! at hundreds of MB/s — much faster than napi can transfer values one
//! at a time. The user-visible `{ columns, rows }` shape is unchanged.
//!
//! ## Format (little-endian throughout)
//!
//! Header:
//!   `LR1\0`           magic, 4 bytes
//!   `u32`             column count C
//!   `for each col:`   `u32` name byte length + UTF-8 bytes
//!   `u32`             row count R
//!
//! Body: R rows × C cells, row-major. Each cell is a `u8` tag + payload:
//!
//! | tag  | type            | payload                                       |
//! |------|-----------------|-----------------------------------------------|
//! | 0x00 | null            | —                                             |
//! | 0x01 | false           | —                                             |
//! | 0x02 | true            | —                                             |
//! | 0x03 | i32             | `i32` LE                                      |
//! | 0x04 | i64             | `i64` LE                                      |
//! | 0x05 | f64             | `f64` LE                                      |
//! | 0x06 | string          | `u32` len + UTF-8 bytes                       |
//! | 0x07 | list            | `u32` n + n cells                             |
//! | 0x08 | map             | `u32` n + n × (`u32` key_len + utf8 + cell)   |
//! | 0x09 | node            | `i64` id                                      |
//! | 0x0A | relationship    | `i64` id                                      |
//! | 0x0B | path            | `u32` nN + nN×`i64` + `u32` nR + nR×`i64`     |
//! | 0x0C | date            | `u32` + UTF-8 ISO                             |
//! | 0x0D | time            | `u32` + UTF-8 ISO                             |
//! | 0x0E | localtime       | `u32` + UTF-8 ISO                             |
//! | 0x0F | datetime        | `u32` + UTF-8 ISO                             |
//! | 0x10 | localdatetime   | `u32` + UTF-8 ISO                             |
//! | 0x11 | duration        | `u32` + UTF-8 ISO                             |
//! | 0x12 | point           | `u8` has_z + `u32` srid + 2 or 3 × `f64`      |
//! | 0x13 | vector          | `u8` coord_type + `u32` dim + values          |
//! | 0x14 | binary          | `u32` n_seg + n_seg × (`u32` len + bytes)     |
//!
//! Vector coord types:
//!   0 = Float64 (8B per value)
//!   1 = Float32 (4B per value)
//!   2 = Integer64 (8B per value)
//!   3 = Integer32 (4B per value)
//!   4 = Integer16 (2B per value)
//!   5 = Integer8 (1B per value)

use lora_database::{LoraValue, Row};
use lora_store::{LoraBinary, LoraPoint, LoraVector, VectorValues};

pub const MAGIC: &[u8; 4] = b"LR1\0";

// ---------------------------------------------------------------------------
// Tags
// ---------------------------------------------------------------------------

pub mod tag {
    pub const NULL: u8 = 0x00;
    pub const FALSE: u8 = 0x01;
    pub const TRUE: u8 = 0x02;
    pub const I32: u8 = 0x03;
    pub const I64: u8 = 0x04;
    pub const F64: u8 = 0x05;
    pub const STRING: u8 = 0x06;
    pub const LIST: u8 = 0x07;
    pub const MAP: u8 = 0x08;
    pub const NODE: u8 = 0x09;
    pub const RELATIONSHIP: u8 = 0x0A;
    pub const PATH: u8 = 0x0B;
    pub const DATE: u8 = 0x0C;
    pub const TIME: u8 = 0x0D;
    pub const LOCAL_TIME: u8 = 0x0E;
    pub const DATETIME: u8 = 0x0F;
    pub const LOCAL_DATETIME: u8 = 0x10;
    pub const DURATION: u8 = 0x11;
    pub const POINT: u8 = 0x12;
    pub const VECTOR: u8 = 0x13;
    pub const BINARY: u8 = 0x14;
}

// ---------------------------------------------------------------------------
// Public entry points
// ---------------------------------------------------------------------------

/// Encode `Vec<Row>` (the engine's native row format) directly into the
/// binary wire format. Skips the `RowArrays` projection, which would
/// otherwise clone every cell and allocate a `Vec<LoraValue>` per row.
///
/// Column names are read from the first row's `iter_named` (only place
/// they're needed). All subsequent rows go through plain `iter()` so
/// we don't pay for the per-cell `Cow<str>` that `iter_named` allocates
/// for anonymous entries.
pub fn encode_query_rows(rows: &[Row]) -> Vec<u8> {
    let column_count = rows.first().map(|r| r.iter().count()).unwrap_or(0);
    let estimated = rows.len() * column_count * 12 + 64;
    let mut buf = Vec::with_capacity(estimated);
    buf.extend_from_slice(MAGIC);
    write_u32(&mut buf, column_count as u32);
    if let Some(first) = rows.first() {
        for (_, name, _) in first.iter_named() {
            write_str(&mut buf, &name);
        }
    }
    write_u32(&mut buf, rows.len() as u32);
    for row in rows {
        for (_, value) in row.iter() {
            encode_value(&mut buf, value);
        }
    }
    buf
}

/// Encode a `{ columns, rows }` payload (RowArrays format) into the
/// binary wire format. Used by transactions which currently still
/// project to RowArrays inside the per-statement loop.
pub fn encode_rows(columns: &[String], rows: &[Vec<LoraValue>]) -> Vec<u8> {
    let estimated = rows.len() * columns.len() * 12 + sum_string_bytes(columns) + 64;
    let mut buf = Vec::with_capacity(estimated);
    buf.extend_from_slice(MAGIC);
    write_u32(&mut buf, columns.len() as u32);
    for col in columns {
        write_str(&mut buf, col);
    }
    write_u32(&mut buf, rows.len() as u32);
    for row in rows {
        for cell in row {
            encode_value(&mut buf, cell);
        }
    }
    buf
}

// ---------------------------------------------------------------------------
// Cell-level encoder
// ---------------------------------------------------------------------------

#[inline]
fn encode_value(buf: &mut Vec<u8>, v: &LoraValue) {
    match v {
        LoraValue::Null => buf.push(tag::NULL),
        LoraValue::Bool(false) => buf.push(tag::FALSE),
        LoraValue::Bool(true) => buf.push(tag::TRUE),
        LoraValue::Int(i) => {
            // Compact: emit i32 when the value fits, else full i64. Saves
            // 4 bytes per cell on the common case (graph IDs, counts,
            // small enums) and a slow BigInt path on the JS decoder.
            if let Ok(n) = i32::try_from(*i) {
                let bytes = n.to_le_bytes();
                buf.extend_from_slice(&[tag::I32, bytes[0], bytes[1], bytes[2], bytes[3]]);
            } else {
                let bytes = i.to_le_bytes();
                buf.extend_from_slice(&[
                    tag::I64,
                    bytes[0],
                    bytes[1],
                    bytes[2],
                    bytes[3],
                    bytes[4],
                    bytes[5],
                    bytes[6],
                    bytes[7],
                ]);
            }
        }
        LoraValue::Float(f) => {
            let bytes = f.to_le_bytes();
            buf.extend_from_slice(&[
                tag::F64,
                bytes[0],
                bytes[1],
                bytes[2],
                bytes[3],
                bytes[4],
                bytes[5],
                bytes[6],
                bytes[7],
            ]);
        }
        LoraValue::String(s) => {
            buf.push(tag::STRING);
            write_str(buf, s);
        }
        LoraValue::List(items) => {
            buf.push(tag::LIST);
            write_u32(buf, items.len() as u32);
            for item in items {
                encode_value(buf, item);
            }
        }
        LoraValue::Map(m) => {
            buf.push(tag::MAP);
            write_u32(buf, m.len() as u32);
            for (k, v) in m {
                write_str(buf, k);
                encode_value(buf, v);
            }
        }
        LoraValue::Node(id) => {
            buf.push(tag::NODE);
            buf.extend_from_slice(&(*id as i64).to_le_bytes());
        }
        LoraValue::Relationship(id) => {
            buf.push(tag::RELATIONSHIP);
            buf.extend_from_slice(&(*id as i64).to_le_bytes());
        }
        LoraValue::Path(p) => {
            buf.push(tag::PATH);
            write_u32(buf, p.nodes.len() as u32);
            for n in &p.nodes {
                buf.extend_from_slice(&(*n as i64).to_le_bytes());
            }
            write_u32(buf, p.rels.len() as u32);
            for r in &p.rels {
                buf.extend_from_slice(&(*r as i64).to_le_bytes());
            }
        }
        LoraValue::Date(d) => write_iso(buf, tag::DATE, &d.to_string()),
        LoraValue::Time(t) => write_iso(buf, tag::TIME, &t.to_string()),
        LoraValue::LocalTime(t) => write_iso(buf, tag::LOCAL_TIME, &t.to_string()),
        LoraValue::DateTime(dt) => write_iso(buf, tag::DATETIME, &dt.to_string()),
        LoraValue::LocalDateTime(dt) => write_iso(buf, tag::LOCAL_DATETIME, &dt.to_string()),
        LoraValue::Duration(d) => write_iso(buf, tag::DURATION, &d.to_string()),
        LoraValue::Point(p) => write_point(buf, p),
        LoraValue::Vector(v) => write_vector(buf, v),
        LoraValue::Binary(b) => write_binary(buf, b),
    }
}

// ---------------------------------------------------------------------------
// Specialised writers
// ---------------------------------------------------------------------------

fn write_iso(buf: &mut Vec<u8>, tag: u8, iso: &str) {
    buf.push(tag);
    write_str(buf, iso);
}

fn write_point(buf: &mut Vec<u8>, p: &LoraPoint) {
    buf.push(tag::POINT);
    buf.push(if p.z.is_some() { 1 } else { 0 });
    write_u32(buf, p.srid);
    buf.extend_from_slice(&p.x.to_le_bytes());
    buf.extend_from_slice(&p.y.to_le_bytes());
    if let Some(z) = p.z {
        buf.extend_from_slice(&z.to_le_bytes());
    }
}

fn write_vector(buf: &mut Vec<u8>, v: &LoraVector) {
    buf.push(tag::VECTOR);
    let coord_tag: u8 = match &v.values {
        VectorValues::Float64(_) => 0,
        VectorValues::Float32(_) => 1,
        VectorValues::Integer64(_) => 2,
        VectorValues::Integer32(_) => 3,
        VectorValues::Integer16(_) => 4,
        VectorValues::Integer8(_) => 5,
    };
    buf.push(coord_tag);
    write_u32(buf, v.dimension as u32);
    match &v.values {
        VectorValues::Float64(vs) => {
            for x in vs {
                buf.extend_from_slice(&x.to_le_bytes());
            }
        }
        VectorValues::Float32(vs) => {
            for x in vs {
                buf.extend_from_slice(&x.to_le_bytes());
            }
        }
        VectorValues::Integer64(vs) => {
            for x in vs {
                buf.extend_from_slice(&x.to_le_bytes());
            }
        }
        VectorValues::Integer32(vs) => {
            for x in vs {
                buf.extend_from_slice(&x.to_le_bytes());
            }
        }
        VectorValues::Integer16(vs) => {
            for x in vs {
                buf.extend_from_slice(&x.to_le_bytes());
            }
        }
        VectorValues::Integer8(vs) => {
            // `i8` and `u8` share size and alignment; one extend
            // beats N pushes. The reinterpret is sound on every
            // target — there's no endianness concern for byte-sized
            // values.
            // SAFETY: `vs.as_ptr()` is valid for `vs.len()` bytes; the
            // target type has identical layout.
            let bytes: &[u8] =
                unsafe { std::slice::from_raw_parts(vs.as_ptr() as *const u8, vs.len()) };
            buf.extend_from_slice(bytes);
        }
    }
}

fn write_binary(buf: &mut Vec<u8>, b: &LoraBinary) {
    buf.push(tag::BINARY);
    let segments = b.segments();
    write_u32(buf, segments.len() as u32);
    for segment in segments {
        write_u32(buf, segment.len() as u32);
        buf.extend_from_slice(segment);
    }
}

// ---------------------------------------------------------------------------
// Primitive helpers
// ---------------------------------------------------------------------------

#[inline(always)]
fn write_u32(buf: &mut Vec<u8>, n: u32) {
    buf.extend_from_slice(&n.to_le_bytes());
}

#[inline(always)]
fn write_str(buf: &mut Vec<u8>, s: &str) {
    write_u32(buf, s.len() as u32);
    buf.extend_from_slice(s.as_bytes());
}

fn sum_string_bytes(strs: &[String]) -> usize {
    strs.iter().map(|s| s.len() + 4).sum()
}
