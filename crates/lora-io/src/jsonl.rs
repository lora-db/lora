//! JSON Lines (NDJSON) codec. One JSON object per line.

use std::io::{BufRead, Write};

use lora_executor::{LoraValue, Row};
use serde_json::Value as J;

use super::format::{
    invalid_data, row_parse_io_error, RowDecoder, RowEncoder, RowParseError, StreamingRowDecoder,
};
use super::value_json::{lora_value_from_json, lora_value_to_json};

pub struct JsonlEncoder<W: Write> {
    writer: W,
}

impl<W: Write> JsonlEncoder<W> {
    pub fn new(writer: W) -> Self {
        Self { writer }
    }

    pub fn into_inner(self) -> W {
        self.writer
    }
}

impl<W: Write> RowEncoder for JsonlEncoder<W> {
    fn begin(&mut self, _columns: &[String]) -> std::io::Result<()> {
        Ok(())
    }

    fn write_row(&mut self, row: &Row) -> std::io::Result<()> {
        let mut obj = serde_json::Map::with_capacity(row.len());
        for (_, name, value) in row.iter_named() {
            obj.insert(name.into_owned(), lora_value_to_json(value));
        }
        serde_json::to_writer(&mut self.writer, &J::Object(obj))?;
        self.writer.write_all(b"\n")
    }

    fn write_named_row(&mut self, columns: &[(String, LoraValue)]) -> std::io::Result<()> {
        let mut obj = serde_json::Map::with_capacity(columns.len());
        for (name, value) in columns {
            obj.insert(name.clone(), lora_value_to_json(value));
        }
        serde_json::to_writer(&mut self.writer, &J::Object(obj))?;
        self.writer.write_all(b"\n")
    }

    fn finish(&mut self) -> std::io::Result<()> {
        self.writer.flush()
    }
}

pub struct JsonlDecoder<R: BufRead> {
    reader: R,
    buf: String,
}

impl<R: BufRead> JsonlDecoder<R> {
    pub fn new(reader: R) -> Self {
        Self {
            reader,
            buf: String::new(),
        }
    }
}

impl<R: BufRead> RowDecoder for JsonlDecoder<R> {
    fn header(&mut self) -> std::io::Result<Option<Vec<String>>> {
        Ok(None)
    }

    fn next_row(&mut self) -> std::io::Result<Option<Vec<(String, LoraValue)>>> {
        loop {
            self.buf.clear();
            let read = self.reader.read_line(&mut self.buf)?;
            if read == 0 {
                return Ok(None);
            }
            let line = self.buf.trim_matches(|c: char| c == '\r' || c == '\n');
            if line.is_empty() {
                continue;
            }
            let v: J = serde_json::from_str(line).map_err(invalid_data)?;
            let J::Object(obj) = v else {
                return Err(invalid_data(format!(
                    "expected JSON object per line, found {}",
                    type_name(&serde_json::from_str(line).unwrap_or(J::Null))
                )));
            };
            let mut out = Vec::with_capacity(obj.len());
            for (k, raw) in obj {
                out.push((k, lora_value_from_json(raw).map_err(invalid_data)?));
            }
            return Ok(Some(out));
        }
    }
}

fn type_name(v: &J) -> &'static str {
    match v {
        J::Null => "null",
        J::Bool(_) => "bool",
        J::Number(_) => "number",
        J::String(_) => "string",
        J::Array(_) => "array",
        J::Object(_) => "object",
    }
}

/// Push-based JSONL decoder. Accumulates bytes between calls to
/// [`Self::feed`], splits on `\n`, parses each completed line as a
/// JSON object, and queues the resulting record for [`Self::drain`].
pub struct StreamingJsonlDecoder {
    /// Bytes received but not yet terminated by a newline. Cleared
    /// every time a `\n` is observed (the line up to and including
    /// it is consumed; the residual tail stays here).
    buffer: Vec<u8>,
    /// Completed records waiting to be drained.
    completed: Vec<Vec<(String, LoraValue)>>,
    bytes_fed: u64,
    rows_emitted: u64,
    /// 1-indexed counter of non-blank lines seen so far. Used to
    /// attribute parse errors to a specific record. Advances even
    /// when a record fails (or is skipped in permissive mode).
    record_index: u64,
    permissive: bool,
    errors: Vec<RowParseError>,
}

impl Default for StreamingJsonlDecoder {
    fn default() -> Self {
        Self::new()
    }
}

impl StreamingJsonlDecoder {
    pub fn new() -> Self {
        Self {
            buffer: Vec::with_capacity(64 * 1024),
            completed: Vec::new(),
            bytes_fed: 0,
            rows_emitted: 0,
            record_index: 0,
            permissive: false,
            errors: Vec::new(),
        }
    }

    fn parse_line(&mut self, line: &[u8]) -> std::io::Result<()> {
        let s = std::str::from_utf8(line).map_err(invalid_data)?;
        let trimmed = s.trim_matches(|c: char| c == '\r' || c == '\n' || c == ' ' || c == '\t');
        if trimmed.is_empty() {
            return Ok(());
        }
        self.record_index += 1;
        match parse_jsonl_object(trimmed) {
            Ok(record) => {
                self.completed.push(record);
                self.rows_emitted += 1;
                Ok(())
            }
            Err(message) => self.report_error(message, trimmed),
        }
    }

    fn report_error(&mut self, message: String, raw: &str) -> std::io::Result<()> {
        let err = RowParseError {
            row: self.record_index,
            column: None,
            raw_sample: RowParseError::make_sample(raw),
            message,
        };
        if self.permissive {
            self.errors.push(err);
            Ok(())
        } else {
            Err(row_parse_io_error(err))
        }
    }
}

fn parse_jsonl_object(line: &str) -> Result<Vec<(String, LoraValue)>, String> {
    let v: J = serde_json::from_str(line).map_err(|e| e.to_string())?;
    let J::Object(obj) = v else {
        return Err(format!(
            "expected JSON object per line, found {}",
            type_name(&v)
        ));
    };
    let mut record = Vec::with_capacity(obj.len());
    for (k, raw) in obj {
        let value = lora_value_from_json(raw).map_err(|e| format!("key `{k}`: {e}"))?;
        record.push((k, value));
    }
    Ok(record)
}

impl StreamingRowDecoder for StreamingJsonlDecoder {
    fn feed(&mut self, chunk: &[u8]) -> std::io::Result<()> {
        if chunk.is_empty() {
            return Ok(());
        }
        self.bytes_fed += chunk.len() as u64;
        self.buffer.extend_from_slice(chunk);

        // Walk the buffer extracting newline-terminated lines.
        // Drain the prefix containing complete lines and re-buffer
        // any partial tail. We collect line spans first so we can
        // borrow `self.buffer` immutably while iterating and only
        // call the (mutating) parser afterwards.
        let mut lines: Vec<Vec<u8>> = Vec::new();
        let mut last_end = 0usize;
        for (idx, &b) in self.buffer.iter().enumerate() {
            if b == b'\n' {
                lines.push(self.buffer[last_end..=idx].to_vec());
                last_end = idx + 1;
            }
        }
        if last_end > 0 {
            self.buffer.drain(..last_end);
        }
        for line in lines {
            self.parse_line(&line)?;
        }
        Ok(())
    }

    fn drain(&mut self) -> std::io::Result<Vec<Vec<(String, LoraValue)>>> {
        Ok(std::mem::take(&mut self.completed))
    }

    fn finish(&mut self) -> std::io::Result<Vec<Vec<(String, LoraValue)>>> {
        // Parse the residual buffer as one final line if it has any
        // non-whitespace content (handles files without a trailing
        // newline).
        let leftover = std::mem::take(&mut self.buffer);
        if !leftover.is_empty() {
            self.parse_line(&leftover)?;
        }
        Ok(std::mem::take(&mut self.completed))
    }

    fn header(&self) -> Option<&[String]> {
        None
    }

    fn bytes_fed(&self) -> u64 {
        self.bytes_fed
    }

    fn rows_emitted(&self) -> u64 {
        self.rows_emitted
    }

    fn set_permissive(&mut self, on: bool) {
        self.permissive = on;
    }

    fn take_errors(&mut self) -> Vec<RowParseError> {
        std::mem::take(&mut self.errors)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn round_trip_scalars() {
        let mut buf = Vec::new();
        {
            let mut enc = JsonlEncoder::new(&mut buf);
            enc.begin(&["name".into(), "age".into()]).unwrap();
            enc.write_named_row(&[
                ("name".into(), LoraValue::String("alice".into())),
                ("age".into(), LoraValue::Int(30)),
            ])
            .unwrap();
            enc.write_named_row(&[
                ("name".into(), LoraValue::String("bob".into())),
                ("age".into(), LoraValue::Int(25)),
            ])
            .unwrap();
            enc.finish().unwrap();
        }
        let text = std::str::from_utf8(&buf).unwrap();
        let lines: Vec<_> = text.lines().collect();
        assert_eq!(lines.len(), 2);
        assert!(lines[0].contains("\"alice\""));

        // serde_json::Map (without preserve_order) sorts keys, so we
        // assert by lookup rather than position.
        let mut dec = JsonlDecoder::new(Cursor::new(buf));
        let r1: std::collections::BTreeMap<_, _> =
            dec.next_row().unwrap().unwrap().into_iter().collect();
        assert_eq!(r1.get("name"), Some(&LoraValue::String("alice".into())));
        assert_eq!(r1.get("age"), Some(&LoraValue::Int(30)));
        let r2: std::collections::BTreeMap<_, _> =
            dec.next_row().unwrap().unwrap().into_iter().collect();
        assert_eq!(r2.get("name"), Some(&LoraValue::String("bob".into())));
        assert_eq!(r2.get("age"), Some(&LoraValue::Int(25)));
        assert!(dec.next_row().unwrap().is_none());
    }

    #[test]
    fn streaming_split_across_chunks() {
        // Half a record, then the rest, then a second record split
        // across two chunks, then a chunk with no newline (carries
        // into finish()).
        let mut dec = StreamingJsonlDecoder::new();
        dec.feed(b"{\"a\":").unwrap();
        assert_eq!(dec.drain().unwrap().len(), 0);
        dec.feed(b"1}\n{\"b\":").unwrap();
        let rows1 = dec.drain().unwrap();
        assert_eq!(rows1.len(), 1);
        assert_eq!(rows1[0][0], ("a".into(), LoraValue::Int(1)));
        dec.feed(b"2}\n{\"c\":3}").unwrap();
        let rows2 = dec.drain().unwrap();
        assert_eq!(rows2.len(), 1);
        assert_eq!(rows2[0][0], ("b".into(), LoraValue::Int(2)));

        // Final unterminated line drained by finish().
        let rows3 = dec.finish().unwrap();
        assert_eq!(rows3.len(), 1);
        assert_eq!(rows3[0][0], ("c".into(), LoraValue::Int(3)));
        assert_eq!(dec.rows_emitted(), 3);
    }

    #[test]
    fn streaming_skips_blank_lines() {
        let mut dec = StreamingJsonlDecoder::new();
        dec.feed(b"\n\n{\"a\":1}\n\n").unwrap();
        let rows = dec.drain().unwrap();
        assert_eq!(rows.len(), 1);
        assert!(dec.finish().unwrap().is_empty());
    }

    #[test]
    fn streaming_strict_mode_bubbles_row_context() {
        let mut dec = StreamingJsonlDecoder::new();
        let err = dec.feed(b"{\"a\":1}\nnot json\n").unwrap_err();
        let parse = super::super::format::downcast_row_parse_error(&err)
            .expect("error should carry RowParseError");
        assert_eq!(parse.row, 2);
        assert!(parse.column.is_none());
        assert!(parse.raw_sample.contains("not json"));
    }

    #[test]
    fn streaming_permissive_mode_skips_and_collects() {
        let mut dec = StreamingJsonlDecoder::new();
        dec.set_permissive(true);
        dec.feed(b"{\"a\":1}\nnot json\n{\"b\":2}\n").unwrap();
        let rows = dec.drain().unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0][0], ("a".into(), LoraValue::Int(1)));
        assert_eq!(rows[1][0], ("b".into(), LoraValue::Int(2)));
        let errors = dec.take_errors();
        assert_eq!(errors.len(), 1);
        assert_eq!(errors[0].row, 2);
        assert_eq!(dec.rows_emitted(), 2);
        // Errors are drained — second call is empty.
        assert!(dec.take_errors().is_empty());
    }

    #[test]
    fn blank_lines_skipped() {
        let input = "\n{\"a\":1}\n\n{\"b\":2}\n";
        let mut dec = JsonlDecoder::new(Cursor::new(input));
        let r1 = dec.next_row().unwrap().unwrap();
        assert_eq!(r1[0], ("a".into(), LoraValue::Int(1)));
        let r2 = dec.next_row().unwrap().unwrap();
        assert_eq!(r2[0], ("b".into(), LoraValue::Int(2)));
        assert!(dec.next_row().unwrap().is_none());
    }
}
