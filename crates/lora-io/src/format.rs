use lora_executor::{LoraValue, Row};

/// Wire format for row-level import/export.
///
/// JSONL is the lossless default — every [`LoraValue`] variant
/// round-trips through the tagged JSON shape produced by
/// [`super::lora_value_to_json`]. CSV is for spreadsheet-friendly
/// scalar data; non-scalar columns are JSON-encoded into a single
/// cell with a `name:json` typed header. The JSON-array variant is
/// the same shape as JSONL but wrapped in a top-level `[ ... ]`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    /// One JSON object per line (`\n`-delimited). Streaming-friendly.
    Jsonl,
    /// A single top-level JSON array of objects.
    Json,
    /// RFC 4180 CSV with optional `name:type` typed headers.
    Csv,
}

impl Format {
    /// Guess the format from a filename suffix. Returns `None` when
    /// the extension isn't recognized.
    pub fn from_extension(name: &str) -> Option<Self> {
        let lower = name.to_ascii_lowercase();
        let ext = lower.rsplit('.').next()?;
        match ext {
            "jsonl" | "ndjson" => Some(Format::Jsonl),
            "json" => Some(Format::Json),
            "csv" => Some(Format::Csv),
            _ => None,
        }
    }

    pub fn content_type(&self) -> &'static str {
        match self {
            Format::Jsonl => "application/x-ndjson",
            Format::Json => "application/json",
            Format::Csv => "text/csv",
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Format::Jsonl => "jsonl",
            Format::Json => "json",
            Format::Csv => "csv",
        }
    }

    /// Parse a lowercase format name. Accepts the same tokens the
    /// playground exposes in its menu.
    pub fn parse(name: &str) -> Option<Self> {
        match name.to_ascii_lowercase().as_str() {
            "jsonl" | "ndjson" => Some(Format::Jsonl),
            "json" => Some(Format::Json),
            "csv" => Some(Format::Csv),
            _ => None,
        }
    }
}

/// Streaming row encoder. Implementors own the underlying writer
/// and may flush at chunk boundaries.
pub trait RowEncoder {
    /// Emit any header bytes the format needs (CSV header row,
    /// opening `[` for the JSON array). Must be called exactly once
    /// before any `write_row`.
    fn begin(&mut self, columns: &[String]) -> std::io::Result<()>;

    /// Write one row. The encoder pulls column names from the row's
    /// [`Row::iter_named`] iterator — they must align with the
    /// columns passed to [`Self::begin`], in the same order, for CSV.
    fn write_row(&mut self, row: &Row) -> std::io::Result<()>;

    /// Write the same shape as [`Self::write_row`] from a flat
    /// `(name, value)` slice. Used by encoders that consume rows
    /// from outside the engine (testing, replay).
    fn write_named_row(&mut self, columns: &[(String, LoraValue)]) -> std::io::Result<()>;

    /// Emit any trailer (closing `]` for the JSON array) and flush
    /// the writer.
    fn finish(&mut self) -> std::io::Result<()>;
}

/// Pull-based row decoder. Reads from a [`std::io::BufRead`] and
/// yields completed rows one at a time. Used by native Rust callers
/// that already have the input as a stream-like reader.
pub trait RowDecoder {
    /// Returns the column names declared in the file header. CSV
    /// uses this; JSONL/JSON return `None` (every row carries its
    /// own keys).
    fn header(&mut self) -> std::io::Result<Option<Vec<String>>>;

    /// Pull the next row as a flat `(name, value)` vector. Returns
    /// `Ok(None)` once the input is exhausted.
    fn next_row(&mut self) -> std::io::Result<Option<Vec<(String, LoraValue)>>>;
}

/// Push-based streaming row decoder. The caller feeds bytes one
/// chunk at a time (e.g. from `File.stream().getReader()` in the
/// browser); the decoder accumulates partial records internally and
/// emits completed rows via [`Self::drain`]. Designed for the WASM
/// streaming-import path where the engine and the source live on
/// different sides of the worker boundary.
///
/// Memory bound: the decoder retains at most one in-progress record
/// plus the bytes between the most-recently-completed record and
/// the end of the most-recently-fed chunk.
pub trait StreamingRowDecoder {
    /// Append a chunk of bytes to the internal buffer and parse any
    /// records that became complete. Idempotent: feeding zero bytes
    /// is a no-op.
    fn feed(&mut self, chunk: &[u8]) -> std::io::Result<()>;

    /// Pull all records completed since the previous `drain` /
    /// `finish` call. Returns an empty `Vec` when no full record
    /// has been parsed yet.
    fn drain(&mut self) -> std::io::Result<Vec<Vec<(String, LoraValue)>>>;

    /// Signal that no more bytes will be fed. Returns any records
    /// emitted by handling the residual buffer — for JSONL/CSV this
    /// covers the case where the file ends without a trailing
    /// newline.
    fn finish(&mut self) -> std::io::Result<Vec<Vec<(String, LoraValue)>>>;

    /// Column names declared in the file header. Populated after
    /// the first record arrives for CSV; always `None` for JSONL.
    fn header(&self) -> Option<&[String]>;

    /// Total bytes accepted via `feed` since construction. Used
    /// for progress reporting.
    fn bytes_fed(&self) -> u64;

    /// Total records emitted via `drain` + `finish` so far.
    fn rows_emitted(&self) -> u64;

    /// Switch the decoder into permissive mode. In permissive mode,
    /// per-record parse failures are accumulated for retrieval via
    /// [`Self::take_errors`] instead of bubbling out of `feed` /
    /// `finish`. Fatal errors (encoding issues that desync the byte
    /// stream itself) still bubble. Must be called before the first
    /// `feed`; later calls take effect at the next record boundary.
    /// Default impl is a no-op for decoders that don't support it.
    fn set_permissive(&mut self, _on: bool) {}

    /// Drain the parse errors accumulated since the previous call.
    /// Empty when permissive mode is off or no failures occurred.
    fn take_errors(&mut self) -> Vec<RowParseError> {
        Vec::new()
    }
}

/// Structured per-record parse failure. Carries the user-visible row
/// number, an optional column attribution (CSV cell parses), a
/// truncated sample of the raw bytes that failed, and the underlying
/// message. Boxed into [`std::io::Error`] via [`row_parse_io_error`]
/// so it travels through the standard `feed` / `finish` return type;
/// recoverable via [`downcast_row_parse_error`].
#[derive(Debug, Clone)]
pub struct RowParseError {
    /// 1-indexed record number. For CSV the header is record 0 and
    /// the first data row is record 1; for JSONL/JSON-array the
    /// first object is record 1. Records that fail mid-parse still
    /// take a slot in this sequence.
    pub row: u64,
    /// Column name when the failure is attributable to a single
    /// cell (CSV typed cell). `None` for whole-record failures and
    /// JSON-based formats.
    pub column: Option<String>,
    /// Truncated raw bytes (or characters) from the offending
    /// record. Capped to keep error payloads bounded.
    pub raw_sample: String,
    /// Human-readable description of what went wrong.
    pub message: String,
}

/// Cap on `raw_sample` length to keep error payloads small enough to
/// transport cheaply across the WASM boundary and render in the UI.
pub const RAW_SAMPLE_MAX_CHARS: usize = 200;

impl RowParseError {
    /// Build a `raw_sample` from the offending text, replacing any
    /// non-printable control characters with `·` (middle dot) and
    /// truncating at [`RAW_SAMPLE_MAX_CHARS`] with a trailing `…`.
    pub fn make_sample(raw: &str) -> String {
        let cleaned: String = raw
            .chars()
            .map(|c| if c.is_control() { '·' } else { c })
            .collect();
        if cleaned.chars().count() <= RAW_SAMPLE_MAX_CHARS {
            cleaned
        } else {
            let truncated: String = cleaned.chars().take(RAW_SAMPLE_MAX_CHARS).collect();
            format!("{truncated}…")
        }
    }

    /// Build a sample from a byte slice, treating non-UTF-8 bytes
    /// with the lossy replacement char.
    pub fn make_sample_from_bytes(raw: &[u8]) -> String {
        Self::make_sample(&String::from_utf8_lossy(raw))
    }
}

impl std::fmt::Display for RowParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "row {}", self.row)?;
        if let Some(col) = &self.column {
            write!(f, ", column `{col}`")?;
        }
        write!(f, ": {}", self.message)?;
        if !self.raw_sample.is_empty() {
            write!(f, " (raw: `{}`)", self.raw_sample)?;
        }
        Ok(())
    }
}

impl std::error::Error for RowParseError {}

/// Wrap a [`RowParseError`] in [`std::io::Error`] so it flows through
/// the existing `Result<_, std::io::Error>` channels. Recover the
/// structured form with [`downcast_row_parse_error`].
pub fn row_parse_io_error(err: RowParseError) -> std::io::Error {
    std::io::Error::new(std::io::ErrorKind::InvalidData, err)
}

/// Attempt to recover a [`RowParseError`] from an `io::Error`
/// previously produced via [`row_parse_io_error`]. Returns `None`
/// for any other error.
pub fn downcast_row_parse_error(err: &std::io::Error) -> Option<&RowParseError> {
    err.get_ref()?.downcast_ref::<RowParseError>()
}

/// Convenience: drive any [`RowEncoder`] through a row iterator and
/// return how many rows were written. Used by [`super::import`] and
/// can also be called directly when a caller already has a
/// materialized result in hand.
pub fn write_all_rows<E, I>(encoder: &mut E, columns: &[String], rows: I) -> std::io::Result<u64>
where
    E: RowEncoder,
    I: IntoIterator<Item = Row>,
{
    encoder.begin(columns)?;
    let mut count = 0u64;
    for row in rows {
        encoder.write_row(&row)?;
        count += 1;
    }
    encoder.finish()?;
    Ok(count)
}

/// Wrap an [`std::io::Error`] with extra context. Used by decoders
/// when surfacing parser errors as I/O errors.
pub fn invalid_data<E: Into<Box<dyn std::error::Error + Send + Sync>>>(err: E) -> std::io::Error {
    std::io::Error::new(std::io::ErrorKind::InvalidData, err)
}
