//! Binary (`lora_binding_buffer`) variants of streaming and transactions.

use std::ffi::{c_char, CStr, CString};
use std::ptr;

use lora_ffi::*;

struct Db(*mut LoraDatabase);

impl Drop for Db {
    fn drop(&mut self) {
        unsafe { lora_db_free(self.0) }
    }
}

fn new_db() -> Db {
    let mut db = ptr::null_mut();
    assert_eq!(unsafe { lora_db_new(&mut db) }, 0);
    Db(db)
}

fn take(s: *mut c_char) -> String {
    if s.is_null() {
        return String::new();
    }
    let out = unsafe { CStr::from_ptr(s) }.to_string_lossy().into_owned();
    unsafe { lora_string_free(s) };
    out
}

fn execute(db: &Db, query: &str) -> String {
    let q = CString::new(query).unwrap();
    let mut result = ptr::null_mut();
    let mut error = ptr::null_mut();
    let status =
        unsafe { lora_db_execute_json(db.0, q.as_ptr(), ptr::null(), &mut result, &mut error) };
    let error = take(error);
    assert_eq!(status, 0, "{query}: {error}");
    take(result)
}

fn open_stream(db: &Db, query: &str) -> *mut LoraQueryStream {
    let q = CString::new(query).unwrap();
    let mut stream = ptr::null_mut();
    let mut error = ptr::null_mut();
    let status =
        unsafe { lora_db_stream_open_json(db.0, q.as_ptr(), ptr::null(), &mut stream, &mut error) };
    let error = take(error);
    assert_eq!(status, 0, "{query}: {error}");
    stream
}

/// A decoded `LR1` result holding only integer cells.
#[derive(Debug)]
struct Decoded {
    columns: Vec<String>,
    rows: Vec<Vec<i64>>,
}

fn u32_at(buf: &[u8], off: &mut usize) -> u32 {
    let v = u32::from_le_bytes(buf[*off..*off + 4].try_into().unwrap());
    *off += 4;
    v
}

fn decode_ints(buf: &[u8]) -> Decoded {
    assert_eq!(&buf[..4], b"LR1\0");
    let mut off = 4;
    let ncols = u32_at(buf, &mut off) as usize;
    let mut columns = Vec::new();
    for _ in 0..ncols {
        let len = u32_at(buf, &mut off) as usize;
        columns.push(String::from_utf8(buf[off..off + len].to_vec()).unwrap());
        off += len;
    }
    let nrows = u32_at(buf, &mut off) as usize;
    let mut rows = Vec::new();
    for _ in 0..nrows {
        let mut row = Vec::new();
        for _ in 0..ncols {
            let tag = buf[off];
            off += 1;
            match tag {
                0x03 => {
                    row.push(i32::from_le_bytes(buf[off..off + 4].try_into().unwrap()) as i64);
                    off += 4;
                }
                0x04 => {
                    row.push(i64::from_le_bytes(buf[off..off + 8].try_into().unwrap()));
                    off += 8;
                }
                other => panic!("unexpected tag {other:#x}"),
            }
        }
        rows.push(row);
    }
    assert_eq!(off, buf.len(), "trailing bytes");
    Decoded { columns, rows }
}

/// Pull one batch: `Ok(None)` at the end, `Err(message)` on an error.
fn next_batch(stream: *mut LoraQueryStream, max_rows: usize) -> Result<Option<Decoded>, String> {
    let mut bytes = ptr::null_mut();
    let mut len = 0;
    let mut error = ptr::null_mut();
    let status =
        unsafe { lora_stream_next_buffer(stream, max_rows, &mut bytes, &mut len, &mut error) };
    let error = take(error);
    if status != 0 {
        assert!(bytes.is_null());
        return Err(error);
    }
    if bytes.is_null() {
        return Ok(None);
    }
    let decoded = decode_ints(unsafe { std::slice::from_raw_parts(bytes, len) });
    unsafe { lora_bytes_free(bytes, len) };
    Ok(Some(decoded))
}

#[test]
fn read_stream_pulls_in_batches() {
    let db = new_db();
    execute(&db, "UNWIND range(1, 1000) AS i CREATE (:N {i: i})");
    let stream = open_stream(&db, "MATCH (n:N) RETURN n.i AS i ORDER BY i");

    let mut sizes = Vec::new();
    let mut seen = Vec::new();
    while let Some(batch) = next_batch(stream, 256).unwrap() {
        assert_eq!(batch.columns, vec!["i".to_string()]);
        sizes.push(batch.rows.len());
        seen.extend(batch.rows.into_iter().map(|r| r[0]));
    }
    assert_eq!(sizes, vec![256, 256, 256, 232]);
    assert_eq!(seen, (1..=1000).collect::<Vec<_>>());
    // Closed: later pulls keep reporting the end.
    assert!(next_batch(stream, 256).unwrap().is_none());
    unsafe { lora_stream_free(stream) };
}

#[test]
fn mutating_stream_batch_never_commits_with_its_last_rows() {
    let db = new_db();
    let query = "UNWIND range(1, 5) AS i CREATE (:M {i: i}) RETURN i";

    // All five rows arrive, but the stream is freed before the pull past
    // the end: it rolls back, exactly like single-row pulls.
    let stream = open_stream(&db, query);
    let mut rows = 0;
    while rows < 5 {
        rows += next_batch(stream, 1000).unwrap().expect("rows").rows.len();
    }
    assert_eq!(rows, 5);
    unsafe { lora_stream_free(stream) };
    assert!(execute(&db, "MATCH (m:M) RETURN count(m) AS c").contains(r#""c":0"#));

    // Pulling past the end commits.
    let stream = open_stream(&db, query);
    let mut rows = 0;
    while let Some(batch) = next_batch(stream, 1000).unwrap() {
        rows += batch.rows.len();
    }
    assert_eq!(rows, 5);
    unsafe { lora_stream_free(stream) };
    assert!(execute(&db, "MATCH (m:M) RETURN count(m) AS c").contains(r#""c":5"#));
}

#[test]
fn transaction_buffer_returns_every_result_in_order() {
    let db = new_db();
    let statements = CString::new(
        r#"[
            {"query": "UNWIND range(1, 3) AS i CREATE (:T {i: i})"},
            {"query": "MATCH (t:T) RETURN t.i AS i ORDER BY i"},
            {"query": "MATCH (t:T) WHERE t.i > 99 RETURN t.i AS none"}
        ]"#,
    )
    .unwrap();
    let mut bytes = ptr::null_mut();
    let mut len = 0;
    let mut error = ptr::null_mut();
    let status = unsafe {
        lora_db_transaction_buffer(
            db.0,
            statements.as_ptr(),
            ptr::null(),
            &mut bytes,
            &mut len,
            &mut error,
        )
    };
    let error = take(error);
    assert_eq!(status, 0, "{error}");
    let buf = unsafe { std::slice::from_raw_parts(bytes, len) }.to_vec();
    unsafe { lora_bytes_free(bytes, len) };

    assert_eq!(&buf[..4], TRANSACTION_BUFFER_MAGIC);
    let mut off = 4;
    assert_eq!(u32_at(&buf, &mut off), 3);
    let mut results = Vec::new();
    for _ in 0..3 {
        let n = u32_at(&buf, &mut off) as usize;
        results.push(decode_ints(&buf[off..off + n]));
        off += n;
    }
    assert_eq!(off, buf.len());
    assert!(results[0].rows.is_empty());
    assert_eq!(results[1].columns, vec!["i".to_string()]);
    assert_eq!(results[1].rows, vec![vec![1], vec![2], vec![3]]);
    // Columns come from the rows, as in the JSON variant: none for an
    // empty result.
    assert!(results[2].columns.is_empty());
    assert!(results[2].rows.is_empty());

    // A failing statement rolls the batch back and reports an error.
    let bad = CString::new(r#"[{"query": "CREATE (:U)"}, {"query": "RETURN nope("}]"#).unwrap();
    let mut error = ptr::null_mut();
    let status = unsafe {
        lora_db_transaction_buffer(
            db.0,
            bad.as_ptr(),
            ptr::null(),
            &mut bytes,
            &mut len,
            &mut error,
        )
    };
    assert_ne!(status, 0);
    assert!(bytes.is_null());
    assert!(!take(error).is_empty());
    assert!(execute(&db, "MATCH (u:U) RETURN count(u) AS c").contains(r#""c":0"#));
}
