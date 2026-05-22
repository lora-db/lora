//! CSV codec with typed-header convention.
//!
//! Headers can be plain (`name`) or typed (`name:int`, `tags:string[]`,
//! `metadata:json`, `:LABEL`, `:ID`, `:START_ID`, `:END_ID`, `:TYPE`).
//! Cells parse according to the header type; cells whose type is
//! `json` are parsed as canonical tagged JSON (so vectors, points,
//! and temporal values can round-trip through CSV).
//!
//! Quoting follows RFC 4180: fields containing `,`, `"`, `\r`, or
//! `\n` are wrapped in double quotes; embedded `"` is escaped by
//! doubling it. Records may use `\n` or `\r\n` separators.

use std::collections::BTreeMap;
use std::io::{BufRead, Write};

use lora_executor::{LoraValue, Row};
use serde_json::Value as J;

use super::format::{
    invalid_data, row_parse_io_error, RowDecoder, RowEncoder, RowParseError, StreamingRowDecoder,
};
use super::value_json::{lora_value_from_json, lora_value_to_json};

/// Logical column type declared in a typed header (`name:type`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CsvType {
    /// No type annotation; treat the cell as a string at decode time
    /// (encoder picks a sensible cell representation).
    Auto,
    String,
    Int,
    Long,
    Float,
    Double,
    Bool,
    Date,
    DateTime,
    LocalDateTime,
    Time,
    LocalTime,
    Duration,
    Point,
    Json,
    /// Element type for `name:T[]` headers. Cells are split on `;`
    /// and each element is parsed as `T`.
    Array(Box<CsvType>),
    /// Schema markers — used by the import driver to route rows into
    /// node vs. relationship targets when no [`super::RowMapping`]
    /// is supplied. The decoder still emits these as plain values
    /// (string for `:LABEL` and `:TYPE`, integer for `:ID` /
    /// `:START_ID` / `:END_ID`).
    SchemaLabel,
    SchemaId,
    SchemaStartId,
    SchemaEndId,
    SchemaType,
}

#[derive(Debug, Clone)]
pub struct CsvHeader {
    /// External column name (no type suffix).
    pub name: String,
    /// Parsed type tag.
    pub ty: CsvType,
}

impl CsvHeader {
    /// Parse one header cell. Returns `Err` for malformed type tags.
    pub fn parse(raw: &str) -> Result<Self, String> {
        let trimmed = raw.trim();
        // Schema markers always start with ':' and have no name.
        if let Some(rest) = trimmed.strip_prefix(':') {
            let ty = match rest.to_ascii_uppercase().as_str() {
                "LABEL" => CsvType::SchemaLabel,
                "ID" => CsvType::SchemaId,
                "START_ID" => CsvType::SchemaStartId,
                "END_ID" => CsvType::SchemaEndId,
                "TYPE" => CsvType::SchemaType,
                other => return Err(format!("unknown schema marker `:{other}`")),
            };
            return Ok(Self {
                name: String::new(),
                ty,
            });
        }

        match trimmed.split_once(':') {
            None => Ok(Self {
                name: trimmed.to_string(),
                ty: CsvType::Auto,
            }),
            Some((name, ty_part)) => Ok(Self {
                name: name.trim().to_string(),
                ty: parse_type(ty_part.trim())?,
            }),
        }
    }
}

fn parse_type(tag: &str) -> Result<CsvType, String> {
    let (base, is_array) = match tag.strip_suffix("[]") {
        Some(b) => (b.trim(), true),
        None => (tag, false),
    };
    let base_ty = match base.to_ascii_lowercase().as_str() {
        "string" | "text" => CsvType::String,
        "int" | "integer" => CsvType::Int,
        "long" => CsvType::Long,
        "float" => CsvType::Float,
        "double" => CsvType::Double,
        "bool" | "boolean" => CsvType::Bool,
        "date" => CsvType::Date,
        "datetime" => CsvType::DateTime,
        "localdatetime" => CsvType::LocalDateTime,
        "time" => CsvType::Time,
        "localtime" => CsvType::LocalTime,
        "duration" => CsvType::Duration,
        "point" => CsvType::Point,
        "json" => CsvType::Json,
        other => return Err(format!("unknown column type `{other}`")),
    };
    Ok(if is_array {
        CsvType::Array(Box::new(base_ty))
    } else {
        base_ty
    })
}

pub struct CsvEncoder<W: Write> {
    writer: W,
    header_written: bool,
    columns: Vec<String>,
    /// Pre-computed type-suffixed header cells (used when caller
    /// supplies explicit types via [`Self::with_types`]).
    typed_headers: Option<Vec<String>>,
}

impl<W: Write> CsvEncoder<W> {
    pub fn new(writer: W) -> Self {
        Self {
            writer,
            header_written: false,
            columns: Vec::new(),
            typed_headers: None,
        }
    }

    /// Pre-declare typed headers (one per column, in order). When set,
    /// [`Self::begin`] uses these directly and ignores `columns`.
    pub fn with_typed_headers(mut self, headers: Vec<String>) -> Self {
        self.typed_headers = Some(headers);
        self
    }

    pub fn into_inner(self) -> W {
        self.writer
    }
}

impl<W: Write> RowEncoder for CsvEncoder<W> {
    fn begin(&mut self, columns: &[String]) -> std::io::Result<()> {
        if self.header_written {
            return Ok(());
        }
        let header_cells: Vec<&str> = if let Some(typed) = &self.typed_headers {
            typed.iter().map(|s| s.as_str()).collect()
        } else {
            columns.iter().map(|s| s.as_str()).collect()
        };
        write_record(&mut self.writer, header_cells.iter().copied())?;
        self.columns = columns.to_vec();
        self.header_written = true;
        Ok(())
    }

    fn write_row(&mut self, row: &Row) -> std::io::Result<()> {
        let mut indexed: BTreeMap<String, &LoraValue> = BTreeMap::new();
        for (_, name, value) in row.iter_named() {
            indexed.insert(name.into_owned(), value);
        }
        let mut cells = Vec::with_capacity(self.columns.len());
        for col in &self.columns {
            let cell = match indexed.get(col.as_str()) {
                Some(v) => encode_cell(v),
                None => encode_cell(&LoraValue::Null),
            };
            cells.push(cell);
        }
        write_record(&mut self.writer, cells.iter().map(|s| s.as_str()))
    }

    fn write_named_row(&mut self, columns: &[(String, LoraValue)]) -> std::io::Result<()> {
        if !self.header_written {
            let header_columns: Vec<String> =
                columns.iter().map(|(name, _)| name.clone()).collect();
            self.begin(&header_columns)?;
        }
        // Contract: every key in `columns` must appear in the header.
        // CSV is positional — once the header is locked, a row can
        // only fill those slots. Unknown keys would otherwise be
        // silently dropped, which is a common source of "where did
        // my column go?" bugs when query shapes drift between calls.
        // The check is debug-only so release builds keep the cheap
        // BTreeMap path; in tests + dev, the panic flags the bug at
        // the call-site that introduced it.
        debug_assert!(
            columns
                .iter()
                .all(|(k, _)| self.columns.iter().any(|c| c == k)),
            "row has keys not in the encoder's header: {:?} (header: {:?})",
            columns.iter().map(|(k, _)| k).collect::<Vec<_>>(),
            self.columns,
        );
        let lookup: BTreeMap<&str, &LoraValue> =
            columns.iter().map(|(k, v)| (k.as_str(), v)).collect();
        let mut cells = Vec::with_capacity(self.columns.len());
        for col in &self.columns {
            let v = lookup
                .get(col.as_str())
                .copied()
                .cloned()
                .unwrap_or(LoraValue::Null);
            cells.push(encode_cell(&v));
        }
        write_record(&mut self.writer, cells.iter().map(|s| s.as_str()))
    }

    fn finish(&mut self) -> std::io::Result<()> {
        self.writer.flush()
    }
}

fn encode_cell(value: &LoraValue) -> String {
    match value {
        LoraValue::Null => String::new(),
        LoraValue::Bool(b) => b.to_string(),
        LoraValue::Int(i) => i.to_string(),
        LoraValue::Float(f) => f.to_string(),
        LoraValue::String(s) => s.clone(),
        LoraValue::List(items) => {
            // Scalar lists are joined with `;`, but only when no
            // element carries a char that the `;`-split decoder would
            // misinterpret — otherwise the round-trip silently loses
            // data (e.g. `["a;b"]` decoding back as two elements).
            // Fall back to JSON for those cases.
            if items.iter().all(is_scalar) && items.iter().all(list_element_safe_for_semicolon) {
                items
                    .iter()
                    .map(encode_cell_scalar_only)
                    .collect::<Vec<_>>()
                    .join(";")
            } else {
                serde_json::to_string(&lora_value_to_json(value)).unwrap_or_default()
            }
        }
        // Temporals / point / vector / binary / map -> tagged JSON.
        _ => serde_json::to_string(&lora_value_to_json(value)).unwrap_or_default(),
    }
}

fn is_scalar(v: &LoraValue) -> bool {
    matches!(
        v,
        LoraValue::Null
            | LoraValue::Bool(_)
            | LoraValue::Int(_)
            | LoraValue::Float(_)
            | LoraValue::String(_)
    )
}

/// String elements containing the `;` separator, embedded quotes, or
/// line terminators can't be encoded safely via `;`-join — the decoder
/// (or downstream CSV consumers) would re-split or mis-quote them. The
/// caller falls back to JSON encoding when this returns `false`.
fn list_element_safe_for_semicolon(v: &LoraValue) -> bool {
    match v {
        LoraValue::String(s) => !s.contains([';', '"', '\n', '\r']),
        _ => true,
    }
}

fn encode_cell_scalar_only(v: &LoraValue) -> String {
    match v {
        LoraValue::Null => String::new(),
        LoraValue::Bool(b) => b.to_string(),
        LoraValue::Int(i) => i.to_string(),
        LoraValue::Float(f) => f.to_string(),
        LoraValue::String(s) => s.clone(),
        _ => String::new(),
    }
}

fn write_record<'a, I, W>(writer: &mut W, cells: I) -> std::io::Result<()>
where
    I: IntoIterator<Item = &'a str>,
    W: Write,
{
    let mut first = true;
    for cell in cells {
        if !first {
            writer.write_all(b",")?;
        }
        first = false;
        write_cell(writer, cell)?;
    }
    writer.write_all(b"\n")
}

fn write_cell<W: Write>(writer: &mut W, value: &str) -> std::io::Result<()> {
    let needs_quote = value
        .chars()
        .any(|c| c == ',' || c == '"' || c == '\n' || c == '\r');
    if !needs_quote {
        writer.write_all(value.as_bytes())
    } else {
        writer.write_all(b"\"")?;
        for ch in value.chars() {
            if ch == '"' {
                writer.write_all(b"\"\"")?;
            } else {
                let mut buf = [0u8; 4];
                writer.write_all(ch.encode_utf8(&mut buf).as_bytes())?;
            }
        }
        writer.write_all(b"\"")
    }
}

pub struct CsvDecoder<R: BufRead> {
    reader: R,
    headers: Option<Vec<CsvHeader>>,
    /// Synthesised column names (with schema markers expanded to a
    /// concrete identifier so the row map has a stable key).
    column_names: Vec<String>,
}

impl<R: BufRead> CsvDecoder<R> {
    pub fn new(reader: R) -> Self {
        Self {
            reader,
            headers: None,
            column_names: Vec::new(),
        }
    }

    /// Expose the parsed typed headers — surfaced by the playground's
    /// mapping editor when previewing a file.
    pub fn parsed_headers(&self) -> Option<&[CsvHeader]> {
        self.headers.as_deref()
    }

    fn ensure_header(&mut self) -> std::io::Result<()> {
        if self.headers.is_some() {
            return Ok(());
        }
        let mut cells = match read_record(&mut self.reader)? {
            Some(c) => c,
            None => {
                self.headers = Some(Vec::new());
                return Ok(());
            }
        };
        if let Some(first) = cells.first_mut() {
            strip_utf8_bom(first);
        }
        let mut headers = Vec::with_capacity(cells.len());
        let mut column_names = Vec::with_capacity(cells.len());
        for (idx, raw) in cells.iter().enumerate() {
            let h = CsvHeader::parse(raw).map_err(invalid_data)?;
            column_names.push(synthetic_name(&h, idx));
            headers.push(h);
        }
        self.headers = Some(headers);
        self.column_names = column_names;
        Ok(())
    }
}

/// Remove the UTF-8 byte-order mark (`\u{feff}`) from the start of a
/// header cell, if present. Excel, Google Sheets, and most native
/// "Save as CSV (UTF-8)" exports prepend the BOM; without this strip
/// the first column name silently includes the BOM character and all
/// downstream `r.name` lookups against the generated Cypher miss.
pub(crate) fn strip_utf8_bom(s: &mut String) {
    if let Some(rest) = s.strip_prefix('\u{feff}') {
        *s = rest.to_string();
    }
}

pub(crate) fn synthetic_name(h: &CsvHeader, idx: usize) -> String {
    if !h.name.is_empty() {
        return h.name.clone();
    }
    match h.ty {
        CsvType::SchemaLabel => "_label".to_string(),
        CsvType::SchemaId => "_id".to_string(),
        CsvType::SchemaStartId => "_start_id".to_string(),
        CsvType::SchemaEndId => "_end_id".to_string(),
        CsvType::SchemaType => "_type".to_string(),
        _ => format!("col_{idx}"),
    }
}

impl<R: BufRead> RowDecoder for CsvDecoder<R> {
    fn header(&mut self) -> std::io::Result<Option<Vec<String>>> {
        self.ensure_header()?;
        Ok(Some(self.column_names.clone()))
    }

    fn next_row(&mut self) -> std::io::Result<Option<Vec<(String, LoraValue)>>> {
        self.ensure_header()?;
        let Some(cells) = read_record(&mut self.reader)? else {
            return Ok(None);
        };
        let headers = self.headers.as_ref().unwrap();
        if cells.len() != headers.len() {
            return Err(invalid_data(format!(
                "row has {} cells; expected {} (one per header)",
                cells.len(),
                headers.len()
            )));
        }
        let mut out = Vec::with_capacity(cells.len());
        for (idx, cell) in cells.into_iter().enumerate() {
            let h = &headers[idx];
            let value = parse_cell(&cell, &h.ty).map_err(invalid_data)?;
            out.push((self.column_names[idx].clone(), value));
        }
        Ok(Some(out))
    }
}

pub(crate) fn parse_cell(raw: &str, ty: &CsvType) -> Result<LoraValue, String> {
    if raw.is_empty() {
        return Ok(LoraValue::Null);
    }
    match ty {
        CsvType::Auto | CsvType::String | CsvType::SchemaLabel | CsvType::SchemaType => {
            Ok(LoraValue::String(raw.to_string()))
        }
        CsvType::Int
        | CsvType::Long
        | CsvType::SchemaId
        | CsvType::SchemaStartId
        | CsvType::SchemaEndId => raw
            .parse::<i64>()
            .map(LoraValue::Int)
            .map_err(|e| format!("invalid integer `{raw}`: {e}")),
        CsvType::Float | CsvType::Double => raw
            .parse::<f64>()
            .map(LoraValue::Float)
            .map_err(|e| format!("invalid float `{raw}`: {e}")),
        CsvType::Bool => match raw.to_ascii_lowercase().as_str() {
            "true" | "t" | "1" | "yes" => Ok(LoraValue::Bool(true)),
            "false" | "f" | "0" | "no" => Ok(LoraValue::Bool(false)),
            other => Err(format!("invalid boolean `{other}`")),
        },
        CsvType::Date
        | CsvType::DateTime
        | CsvType::LocalDateTime
        | CsvType::Time
        | CsvType::LocalTime
        | CsvType::Duration
        | CsvType::Point => {
            // Accept either tagged JSON or an ISO string. Try tagged
            // first by sniffing for `{`; fall back to the typed string.
            if raw.trim_start().starts_with('{') {
                let v: J = serde_json::from_str(raw).map_err(|e| e.to_string())?;
                lora_value_from_json(v)
            } else {
                let tag = match ty {
                    CsvType::Date => "date",
                    CsvType::DateTime => "datetime",
                    CsvType::LocalDateTime => "localdatetime",
                    CsvType::Time => "time",
                    CsvType::LocalTime => "localtime",
                    CsvType::Duration => "duration",
                    CsvType::Point => {
                        return Err("point cells must be JSON-encoded".into());
                    }
                    _ => unreachable!(),
                };
                let json = serde_json::json!({ "kind": tag, "iso": raw });
                lora_value_from_json(json)
            }
        }
        CsvType::Json => {
            let v: J = serde_json::from_str(raw).map_err(|e| e.to_string())?;
            lora_value_from_json(v)
        }
        CsvType::Array(inner) => {
            // ';' separator; an explicit `[...]` JSON cell also works.
            if raw.trim_start().starts_with('[') {
                let v: J = serde_json::from_str(raw).map_err(|e| e.to_string())?;
                lora_value_from_json(v)
            } else {
                let mut items = Vec::new();
                for part in raw.split(';') {
                    items.push(parse_cell(part, inner)?);
                }
                Ok(LoraValue::List(items))
            }
        }
    }
}

/// Read one CSV record. Returns `Ok(None)` at EOF.
fn read_record<R: BufRead>(reader: &mut R) -> std::io::Result<Option<Vec<String>>> {
    let mut cells: Vec<Vec<u8>> = Vec::new();
    let mut current: Vec<u8> = Vec::new();
    let mut in_quotes = false;
    let mut started = false;

    loop {
        let buf = reader.fill_buf()?;
        if buf.is_empty() {
            if !started && current.is_empty() && cells.is_empty() {
                return Ok(None);
            }
            cells.push(std::mem::take(&mut current));
            return decode_record(cells).map(Some);
        }
        let consumed = process_buf(buf, &mut cells, &mut current, &mut in_quotes, &mut started);
        let (n, end_of_record) = consumed;
        reader.consume(n);
        if end_of_record {
            cells.push(std::mem::take(&mut current));
            return decode_record(cells).map(Some);
        }
    }
}

fn decode_record(cells: Vec<Vec<u8>>) -> std::io::Result<Vec<String>> {
    cells
        .into_iter()
        .map(|cell| String::from_utf8(cell).map_err(invalid_data))
        .collect()
}

/// Inner record-parsing state machine. Mutates `cells`/`current` as it
/// consumes bytes from `buf`. Returns `(bytes_consumed, end_of_record)`.
///
/// Re-used by [`StreamingCsvDecoder`] to drive a push-based parser:
/// the state lives on the decoder struct rather than the stack, and
/// each `feed(chunk)` call advances it one chunk at a time.
pub(crate) fn process_buf(
    buf: &[u8],
    cells: &mut Vec<Vec<u8>>,
    current: &mut Vec<u8>,
    in_quotes: &mut bool,
    started: &mut bool,
) -> (usize, bool) {
    let mut i = 0;
    while i < buf.len() {
        let b = buf[i];
        *started = true;
        if *in_quotes {
            match b {
                b'"' => {
                    if i + 1 < buf.len() && buf[i + 1] == b'"' {
                        current.push(b'"');
                        i += 2;
                    } else {
                        *in_quotes = false;
                        i += 1;
                    }
                }
                _ => {
                    current.push(b);
                    i += 1;
                }
            }
        } else {
            match b {
                b'"' => {
                    *in_quotes = true;
                    i += 1;
                }
                b',' => {
                    cells.push(std::mem::take(current));
                    i += 1;
                }
                b'\n' => {
                    return (i + 1, true);
                }
                b'\r' => {
                    let consumed = if i + 1 < buf.len() && buf[i + 1] == b'\n' {
                        i + 2
                    } else {
                        i + 1
                    };
                    return (consumed, true);
                }
                _ => {
                    current.push(b);
                    i += 1;
                }
            }
        }
    }
    (i, false)
}

/// Push-based CSV decoder. Layers a streaming feed loop on top of
/// the same `process_buf` byte-level state machine used by the
/// pull-based [`CsvDecoder`]. The cell/record state lives on the
/// struct so it persists across `feed` calls — partial records
/// stay parked until enough bytes arrive to complete them.
pub struct StreamingCsvDecoder {
    /// Bytes received but not yet consumed by the state machine.
    /// `process_buf` reports how many bytes it consumed per call;
    /// anything left over (inside a quoted cell, mid-cell, etc.)
    /// stays here for the next feed.
    chunk_buffer: Vec<u8>,
    /// In-flight record cells.
    cells: Vec<Vec<u8>>,
    /// In-flight cell.
    current: Vec<u8>,
    in_quotes: bool,
    started: bool,
    /// Parsed header row, populated after the first record completes.
    /// `None` before the header is seen.
    header: Option<Vec<CsvHeader>>,
    /// Synthesised column names (header names canonicalised so
    /// `:ID` becomes `_id`, etc.).
    column_names: Vec<String>,
    /// Completed data records waiting for `drain`.
    completed: Vec<Vec<(String, LoraValue)>>,
    bytes_fed: u64,
    rows_emitted: u64,
    /// 1-indexed counter of data rows seen (excluding the header).
    /// Advances even when a row fails or is skipped in permissive
    /// mode, so error attribution always reports the user-visible
    /// row number.
    record_index: u64,
    permissive: bool,
    errors: Vec<RowParseError>,
}

impl Default for StreamingCsvDecoder {
    fn default() -> Self {
        Self::new()
    }
}

impl StreamingCsvDecoder {
    pub fn new() -> Self {
        Self {
            chunk_buffer: Vec::with_capacity(64 * 1024),
            cells: Vec::new(),
            current: Vec::new(),
            in_quotes: false,
            started: false,
            header: None,
            column_names: Vec::new(),
            completed: Vec::new(),
            bytes_fed: 0,
            rows_emitted: 0,
            record_index: 0,
            permissive: false,
            errors: Vec::new(),
        }
    }

    /// Parsed typed headers, surfaced once the first record has
    /// been consumed. Useful for the playground's mapping preview.
    pub fn parsed_headers(&self) -> Option<&[CsvHeader]> {
        self.header.as_deref()
    }

    /// Drain as many complete records as the current buffer holds.
    fn advance(&mut self) -> std::io::Result<()> {
        // Repeatedly call process_buf until it stops finding record
        // terminators. Each call advances the byte cursor by
        // `consumed` bytes; we drain that prefix and loop.
        loop {
            let (consumed, end_of_record) = process_buf(
                &self.chunk_buffer,
                &mut self.cells,
                &mut self.current,
                &mut self.in_quotes,
                &mut self.started,
            );
            if consumed > 0 {
                self.chunk_buffer.drain(..consumed);
            }
            if !end_of_record {
                break;
            }
            // Record complete: capture the current cell and route
            // the record (header → metadata, body → completed list).
            self.cells.push(std::mem::take(&mut self.current));
            let record = decode_record(std::mem::take(&mut self.cells))?;
            self.handle_record(record)?;
        }
        Ok(())
    }

    fn handle_record(&mut self, mut cells: Vec<String>) -> std::io::Result<()> {
        if self.header.is_none() {
            // Header errors are fatal: without a valid header the
            // body can't be parsed at all, so permissive mode does
            // not apply.
            if let Some(first) = cells.first_mut() {
                strip_utf8_bom(first);
            }
            let mut headers = Vec::with_capacity(cells.len());
            let mut column_names = Vec::with_capacity(cells.len());
            for (idx, raw) in cells.iter().enumerate() {
                let h = CsvHeader::parse(raw).map_err(invalid_data)?;
                column_names.push(synthetic_name(&h, idx));
                headers.push(h);
            }
            self.header = Some(headers);
            self.column_names = column_names;
            return Ok(());
        }
        self.record_index += 1;
        let header = self.header.as_ref().unwrap();
        if cells.len() != header.len() {
            let message = format!(
                "row has {} cells; expected {} (one per header)",
                cells.len(),
                header.len()
            );
            return self.report_record_error(None, message, &cells);
        }
        let mut out = Vec::with_capacity(cells.len());
        for (idx, cell) in cells.iter().enumerate() {
            let h = &header[idx];
            match parse_cell(cell, &h.ty) {
                Ok(value) => out.push((self.column_names[idx].clone(), value)),
                Err(message) => {
                    let column = Some(self.column_names[idx].clone());
                    return self.report_record_error(column, message, &cells);
                }
            }
        }
        self.completed.push(out);
        self.rows_emitted += 1;
        Ok(())
    }

    fn report_record_error(
        &mut self,
        column: Option<String>,
        message: String,
        cells: &[String],
    ) -> std::io::Result<()> {
        let raw = cells.join(",");
        let err = RowParseError {
            row: self.record_index,
            column,
            raw_sample: RowParseError::make_sample(&raw),
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

impl StreamingRowDecoder for StreamingCsvDecoder {
    fn feed(&mut self, chunk: &[u8]) -> std::io::Result<()> {
        if chunk.is_empty() {
            return Ok(());
        }
        self.bytes_fed += chunk.len() as u64;
        self.chunk_buffer.extend_from_slice(chunk);
        self.advance()
    }

    fn drain(&mut self) -> std::io::Result<Vec<Vec<(String, LoraValue)>>> {
        Ok(std::mem::take(&mut self.completed))
    }

    fn finish(&mut self) -> std::io::Result<Vec<Vec<(String, LoraValue)>>> {
        // Flush anything still in the byte buffer.
        self.advance()?;
        // Files often end without a trailing newline; if a record
        // was building when input ran out, close it now.
        if self.started && (!self.cells.is_empty() || !self.current.is_empty() || self.in_quotes) {
            self.cells.push(std::mem::take(&mut self.current));
            let record = decode_record(std::mem::take(&mut self.cells))?;
            self.started = false;
            self.in_quotes = false;
            self.handle_record(record)?;
        }
        Ok(std::mem::take(&mut self.completed))
    }

    fn header(&self) -> Option<&[String]> {
        if self.column_names.is_empty() {
            None
        } else {
            Some(&self.column_names)
        }
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
    fn round_trip_simple() {
        let mut buf = Vec::new();
        {
            let mut enc = CsvEncoder::new(&mut buf);
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
        assert_eq!(text, "name,age\nalice,30\nbob,25\n");

        let mut dec = CsvDecoder::new(Cursor::new(buf));
        let h = dec.header().unwrap().unwrap();
        assert_eq!(h, vec!["name".to_string(), "age".to_string()]);

        let r1 = dec.next_row().unwrap().unwrap();
        assert_eq!(r1[0].1, LoraValue::String("alice".into()));
        // Auto type -> string. Caller would re-tag via column type.
        assert_eq!(r1[1].1, LoraValue::String("30".into()));
    }

    #[test]
    fn typed_headers_parse_numeric_cells() {
        let csv = "name:string,age:int\nalice,30\nbob,25\n";
        let mut dec = CsvDecoder::new(Cursor::new(csv));
        let h = dec.header().unwrap().unwrap();
        assert_eq!(h, vec!["name".to_string(), "age".to_string()]);
        let r = dec.next_row().unwrap().unwrap();
        assert_eq!(r[0].1, LoraValue::String("alice".into()));
        assert_eq!(r[1].1, LoraValue::Int(30));
    }

    #[test]
    fn schema_markers_parse() {
        let csv = ":ID,:LABEL,name:string\n1,User,alice\n2,User,bob\n";
        let mut dec = CsvDecoder::new(Cursor::new(csv));
        let h = dec.header().unwrap().unwrap();
        assert_eq!(
            h,
            vec!["_id".to_string(), "_label".to_string(), "name".to_string()]
        );
        let r = dec.next_row().unwrap().unwrap();
        assert_eq!(r[0].1, LoraValue::Int(1));
        assert_eq!(r[1].1, LoraValue::String("User".into()));
    }

    #[test]
    fn array_cells_split_on_semicolon() {
        let csv = "tags:string[]\nfoo;bar;baz\n";
        let mut dec = CsvDecoder::new(Cursor::new(csv));
        let _ = dec.header().unwrap();
        let r = dec.next_row().unwrap().unwrap();
        let LoraValue::List(items) = &r[0].1 else {
            panic!("expected list");
        };
        assert_eq!(items.len(), 3);
        assert_eq!(items[0], LoraValue::String("foo".into()));
    }

    #[test]
    fn quoted_cells_preserve_commas_and_newlines() {
        let csv = "name:string,note:string\n\"hi, there\",\"line1\nline2\"\n";
        let mut dec = CsvDecoder::new(Cursor::new(csv));
        let _ = dec.header().unwrap();
        let r = dec.next_row().unwrap().unwrap();
        assert_eq!(r[0].1, LoraValue::String("hi, there".into()));
        assert_eq!(r[1].1, LoraValue::String("line1\nline2".into()));
    }

    #[test]
    fn quotes_escape_by_doubling() {
        let csv = "v:string\n\"she said \"\"hi\"\"\"\n";
        let mut dec = CsvDecoder::new(Cursor::new(csv));
        let _ = dec.header().unwrap();
        let r = dec.next_row().unwrap().unwrap();
        assert_eq!(r[0].1, LoraValue::String("she said \"hi\"".into()));
    }

    #[test]
    fn encoder_quotes_when_needed() {
        let mut buf = Vec::new();
        {
            let mut enc = CsvEncoder::new(&mut buf);
            enc.begin(&["v".into()]).unwrap();
            enc.write_named_row(&[("v".into(), LoraValue::String("hi, \"there\"\n".into()))])
                .unwrap();
            enc.finish().unwrap();
        }
        let text = std::str::from_utf8(&buf).unwrap();
        assert_eq!(text, "v\n\"hi, \"\"there\"\"\n\"\n");
    }

    #[test]
    fn json_typed_cells_round_trip_temporal() {
        let csv = "ts:datetime\n2024-01-15T10:30:00Z\n";
        let mut dec = CsvDecoder::new(Cursor::new(csv));
        let _ = dec.header().unwrap();
        let r = dec.next_row().unwrap().unwrap();
        match &r[0].1 {
            LoraValue::DateTime(_) => {}
            other => panic!("expected DateTime, got {other:?}"),
        }
    }

    #[test]
    fn streaming_csv_split_across_chunks() {
        // Header split mid-cell, multiple rows split arbitrarily.
        let mut dec = StreamingCsvDecoder::new();
        dec.feed(b"name:s").unwrap();
        assert!(dec.header().is_none());
        dec.feed(b"tring,age:int\nalice,30\nbo").unwrap();
        let rows = dec.drain().unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(
            rows[0][0],
            ("name".into(), LoraValue::String("alice".into()))
        );
        assert_eq!(rows[0][1], ("age".into(), LoraValue::Int(30)));
        dec.feed(b"b,25\n").unwrap();
        let rows2 = dec.drain().unwrap();
        assert_eq!(rows2.len(), 1);
        assert_eq!(
            rows2[0][0],
            ("name".into(), LoraValue::String("bob".into()))
        );
        // Header should be populated now.
        assert_eq!(
            dec.header().unwrap(),
            &["name".to_string(), "age".to_string()]
        );
        // No trailing newline on final record: dripped through finish().
        dec.feed(b"carol,40").unwrap();
        assert!(dec.drain().unwrap().is_empty());
        let final_rows = dec.finish().unwrap();
        assert_eq!(final_rows.len(), 1);
        assert_eq!(
            final_rows[0][0],
            ("name".into(), LoraValue::String("carol".into()))
        );
        assert_eq!(dec.rows_emitted(), 3);
    }

    #[test]
    fn streaming_csv_quoted_newline_across_chunks() {
        // A quoted cell with an embedded \n that gets split mid-cell
        // across feeds. The state machine must keep `in_quotes` set
        // across calls so the newline doesn't terminate the record
        // prematurely.
        let mut dec = StreamingCsvDecoder::new();
        dec.feed(b"v:string\n\"line").unwrap();
        assert!(dec.drain().unwrap().is_empty());
        dec.feed(b"1\nline2\"\n").unwrap();
        let rows = dec.drain().unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(
            rows[0][0],
            ("v".into(), LoraValue::String("line1\nline2".into()))
        );
    }

    #[test]
    fn streaming_csv_utf8_split_across_chunks() {
        let mut dec = StreamingCsvDecoder::new();
        dec.feed(b"name:string\nal").unwrap();
        dec.feed(&[0xc3]).unwrap();
        dec.feed(&[0xa9, b'\n']).unwrap();
        let rows = dec.drain().unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0][0], ("name".into(), LoraValue::String("alé".into())));
    }

    #[test]
    fn streaming_csv_strict_attributes_failing_cell() {
        let mut dec = StreamingCsvDecoder::new();
        let err = dec
            .feed(b"name:string,age:int\nalice,30\nbob,not-a-number\n")
            .unwrap_err();
        let parse = super::super::format::downcast_row_parse_error(&err)
            .expect("error should carry RowParseError");
        assert_eq!(parse.row, 2);
        assert_eq!(parse.column.as_deref(), Some("age"));
        assert!(parse.message.contains("not-a-number"));
        assert!(parse.raw_sample.contains("bob"));
    }

    #[test]
    fn streaming_csv_permissive_skips_bad_rows() {
        let mut dec = StreamingCsvDecoder::new();
        dec.set_permissive(true);
        dec.feed(b"name:string,age:int\nalice,30\nbob,oops\ncarol,40\n")
            .unwrap();
        let rows = dec.drain().unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(
            rows[0][0],
            ("name".into(), LoraValue::String("alice".into()))
        );
        assert_eq!(
            rows[1][0],
            ("name".into(), LoraValue::String("carol".into()))
        );
        let errors = dec.take_errors();
        assert_eq!(errors.len(), 1);
        assert_eq!(errors[0].row, 2);
        assert_eq!(errors[0].column.as_deref(), Some("age"));
    }

    #[test]
    fn pull_decoder_strips_utf8_bom_from_first_header() {
        // Mimics what Excel / Google Sheets writes on "Save as CSV
        // (UTF-8)": the file starts with `EF BB BF`. Without the
        // strip, the first column name parses as `\u{feff}name` and
        // every downstream lookup misses.
        let bytes = [0xEF, 0xBB, 0xBF];
        let mut csv = String::from_utf8(bytes.to_vec()).unwrap();
        csv.push_str("name,age\nalice,30\n");
        let mut dec = CsvDecoder::new(Cursor::new(csv));
        let h = dec.header().unwrap().unwrap();
        assert_eq!(h, vec!["name".to_string(), "age".to_string()]);
        let r = dec.next_row().unwrap().unwrap();
        assert_eq!(r[0].0, "name");
    }

    #[test]
    fn streaming_csv_strips_utf8_bom_from_first_header() {
        let mut dec = StreamingCsvDecoder::new();
        // BOM split across the chunk boundary on purpose — the strip
        // happens after the whole first record is assembled, so the
        // exact chunking of the BOM bytes doesn't matter.
        dec.feed(&[0xEF, 0xBB]).unwrap();
        dec.feed(&[0xBF]).unwrap();
        dec.feed(b"name,age\nalice,30\n").unwrap();
        let rows = dec.drain().unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0][0].0, "name");
        assert_eq!(
            dec.header().unwrap(),
            &["name".to_string(), "age".to_string()]
        );
    }

    #[test]
    fn encoder_falls_back_to_json_when_list_element_contains_separator() {
        // `["a;b", "c"]` would round-trip as `["a", "b", "c"]` if
        // we picked the `;`-join encoding — the decoder splits on
        // `;`. Force the JSON path instead so the data survives.
        let mut buf = Vec::new();
        {
            let mut enc = CsvEncoder::new(&mut buf);
            enc.begin(&["tags".into()]).unwrap();
            enc.write_named_row(&[(
                "tags".into(),
                LoraValue::List(vec![
                    LoraValue::String("a;b".into()),
                    LoraValue::String("c".into()),
                ]),
            )])
            .unwrap();
            enc.finish().unwrap();
        }
        let text = std::str::from_utf8(&buf).unwrap();
        // Cell carries the JSON encoding, comma-quoted because of the
        // embedded `"`.
        assert!(
            text.contains(r#""[""a;b"",""c""]""#),
            "expected JSON-encoded list, got: {text}"
        );
    }

    #[test]
    fn encoder_uses_semicolon_join_for_safe_list_elements() {
        let mut buf = Vec::new();
        {
            let mut enc = CsvEncoder::new(&mut buf);
            enc.begin(&["tags".into()]).unwrap();
            enc.write_named_row(&[(
                "tags".into(),
                LoraValue::List(vec![
                    LoraValue::String("a".into()),
                    LoraValue::String("b".into()),
                ]),
            )])
            .unwrap();
            enc.finish().unwrap();
        }
        let text = std::str::from_utf8(&buf).unwrap();
        assert_eq!(text, "tags\na;b\n");
    }

    #[test]
    fn streaming_csv_permissive_handles_cell_count_mismatch() {
        let mut dec = StreamingCsvDecoder::new();
        dec.set_permissive(true);
        dec.feed(b"a:string,b:string\nx,y\nz\nq,r\n").unwrap();
        let rows = dec.drain().unwrap();
        assert_eq!(rows.len(), 2);
        let errors = dec.take_errors();
        assert_eq!(errors.len(), 1);
        assert_eq!(errors[0].row, 2);
        assert!(errors[0].column.is_none());
        assert!(errors[0].message.contains("cells"));
    }
}
