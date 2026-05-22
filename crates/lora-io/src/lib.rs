//! Row-level data I/O codecs for LoraDB.
//!
//! `lora-io` is the row-level counterpart to `lora-snapshot`. Snapshots
//! ship the entire graph as a binary blob; `lora-io` ships **rows**
//! as JSONL, JSON, or CSV. Encoders consume native
//! [`lora_executor::Row`] values and stream them to any
//! [`std::io::Write`]; decoders parse incoming bytes back into flat
//! `(name, LoraValue)` records ready to feed into Cypher `CREATE`
//! statements (see [`RowMapping`]).
//!
//! The crate has no dependency on the database engine itself. It
//! deals only with the value model and the on-disk formats. The
//! import/export *driver* (which actually executes Cypher) lives in
//! `lora-database::io` and re-uses these codecs.

mod csv;
mod format;
mod json_array;
mod jsonl;
mod mapping;
mod value_json;

pub use csv::{CsvDecoder, CsvEncoder, StreamingCsvDecoder};
pub use format::{
    downcast_row_parse_error, invalid_data, row_parse_io_error, write_all_rows, Format, RowDecoder,
    RowEncoder, RowParseError, StreamingRowDecoder, RAW_SAMPLE_MAX_CHARS,
};
pub use json_array::{JsonArrayDecoder, JsonArrayEncoder, StreamingJsonArrayDecoder};
pub use jsonl::{JsonlDecoder, JsonlEncoder, StreamingJsonlDecoder};
pub use mapping::{
    parameterized_create_for_node, parameterized_create_for_relationship, ColumnSpec, RowMapping,
};
pub use value_json::{lora_value_from_json, lora_value_to_json};
