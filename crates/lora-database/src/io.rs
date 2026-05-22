//! Row-level bulk import / export driver.
//!
//! Thin orchestration on top of [`lora_io`]. Export drives the existing
//! [`QueryStream`] through a [`lora_io::RowEncoder`]; import drives a
//! [`lora_io::RowDecoder`] through batched
//! `UNWIND $rows AS r CREATE …` statements either generated from a
//! [`lora_io::RowMapping`] (auto-mapping path) or supplied verbatim by
//! the caller (Cypher template escape hatch).

use std::any::Any;
use std::collections::BTreeMap;
use std::io::{BufRead, Write};

use lora_executor::{ExecuteOptions, LoraValue, QueryResult, ResultFormat};
use lora_io::{
    CsvDecoder, CsvEncoder, Format, JsonArrayDecoder, JsonArrayEncoder, JsonlDecoder, JsonlEncoder,
    RowDecoder, RowEncoder, RowMapping,
};
use lora_store::{GraphStorage, GraphStorageMut, InMemoryGraph};

use crate::error::{LoraError, LoraErrorCode};
use crate::Database;

/// Rows-shipped accounting from an export run.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct ExportStats {
    pub rows: u64,
}

/// Rows-shipped accounting from an import run.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct ImportStats {
    pub rows: u64,
    pub batches: u64,
}

/// Default batch size for [`Database::import_rows`] /
/// [`Database::import_with_template`] when none is supplied. Sized to
/// keep `$rows` parameter payloads roughly bounded for typical row widths.
pub const DEFAULT_IMPORT_BATCH_SIZE: usize = 1_000;

impl<S> Database<S>
where
    S: GraphStorage + GraphStorageMut + Any + Clone + Send + Sync + 'static,
{
    /// Encode a query's results into `writer`. Materialises the full
    /// result set first (`RowArrays` projection) before encoding, so
    /// peak memory is `O(result rows)`. Works on any storage backend.
    ///
    /// For the in-memory backend prefer
    /// [`Database::export_query_streaming`], which pulls rows from
    /// the engine's true streaming cursor and keeps peak memory
    /// bounded by the encoder buffer.
    pub fn export_query<W: Write>(
        &self,
        query: &str,
        params: BTreeMap<String, LoraValue>,
        format: Format,
        writer: W,
    ) -> Result<ExportStats, LoraError> {
        match format {
            Format::Jsonl => self.drive_export(query, params, JsonlEncoder::new(writer)),
            Format::Json => self.drive_export(query, params, JsonArrayEncoder::new(writer)),
            Format::Csv => self.drive_export(query, params, CsvEncoder::new(writer)),
        }
    }

    fn drive_export<E: RowEncoder>(
        &self,
        query: &str,
        params: BTreeMap<String, LoraValue>,
        mut encoder: E,
    ) -> Result<ExportStats, LoraError> {
        // Use the RowArrays result format so we get plan-derived
        // column ordering even when the result set is empty. The
        // executor materializes rows once; we drive them through
        // the encoder row-at-a-time so memory stays bounded by the
        // result set, not by the on-disk output size.
        let result = self.execute_with_params(
            query,
            Some(ExecuteOptions {
                format: ResultFormat::RowArrays,
            }),
            params,
        )?;
        let QueryResult::RowArrays(arrays) = result else {
            return Err(LoraError::new(
                LoraErrorCode::Internal,
                "expected RowArrays result for export".to_string(),
            ));
        };
        encoder.begin(&arrays.columns).map_err(io_err)?;
        let mut rows = 0u64;
        for row_vals in arrays.rows {
            let pairs: Vec<(String, LoraValue)> =
                arrays.columns.iter().cloned().zip(row_vals).collect();
            encoder.write_named_row(&pairs).map_err(io_err)?;
            rows += 1;
        }
        encoder.finish().map_err(io_err)?;
        Ok(ExportStats { rows })
    }

    /// Decode rows from `reader` and apply them to the graph via the
    /// supplied [`RowMapping`]. The mapping renders a parameterised
    /// `UNWIND $rows AS r CREATE …` template; rows are batched and
    /// each batch executes as a single auto-committed statement.
    pub fn import_rows<R: BufRead>(
        &self,
        reader: R,
        format: Format,
        mapping: &RowMapping,
        batch_size: Option<usize>,
    ) -> Result<ImportStats, LoraError> {
        let template = mapping.to_cypher().map_err(|e| {
            LoraError::new(
                LoraErrorCode::InvalidParams,
                format!("invalid row mapping: {e}"),
            )
        })?;
        self.import_with_template(reader, format, &template, batch_size)
    }

    /// Decode rows from `reader` and execute `template` once per
    /// batch with `$rows` bound to that batch's row objects. The
    /// caller-supplied template is the escape hatch for the
    /// auto-mapping path — anything Cypher can express is fair game.
    pub fn import_with_template<R: BufRead>(
        &self,
        reader: R,
        format: Format,
        template: &str,
        batch_size: Option<usize>,
    ) -> Result<ImportStats, LoraError> {
        let batch = batch_size.unwrap_or(DEFAULT_IMPORT_BATCH_SIZE).max(1);
        match format {
            Format::Jsonl => self.drive_import(template, JsonlDecoder::new(reader), batch),
            Format::Json => self.drive_import(template, JsonArrayDecoder::new(reader), batch),
            Format::Csv => self.drive_import(template, CsvDecoder::new(reader), batch),
        }
    }

    fn drive_import<D: RowDecoder>(
        &self,
        template: &str,
        mut decoder: D,
        batch_size: usize,
    ) -> Result<ImportStats, LoraError> {
        // Eagerly read the header so format errors surface before any
        // mutations happen.
        decoder.header().map_err(io_err)?;

        let mut stats = ImportStats::default();
        let mut buf: Vec<LoraValue> = Vec::with_capacity(batch_size);

        while let Some(cells) = decoder.next_row().map_err(io_err)? {
            let row_map: BTreeMap<String, LoraValue> = cells.into_iter().collect();
            buf.push(LoraValue::Map(row_map));
            if buf.len() >= batch_size {
                self.flush_batch(template, &mut buf, &mut stats)?;
            }
        }
        if !buf.is_empty() {
            self.flush_batch(template, &mut buf, &mut stats)?;
        }
        Ok(stats)
    }

    fn flush_batch(
        &self,
        template: &str,
        buf: &mut Vec<LoraValue>,
        stats: &mut ImportStats,
    ) -> Result<(), LoraError> {
        let batch = std::mem::take(buf);
        let batch_len = batch.len() as u64;
        let mut params = BTreeMap::new();
        params.insert("rows".to_string(), LoraValue::List(batch));
        self.execute_with_params(template, None, params)?;
        stats.rows += batch_len;
        stats.batches += 1;
        Ok(())
    }
}

fn io_err(err: std::io::Error) -> LoraError {
    LoraError::with_source(LoraErrorCode::Io, format!("{err}"), err)
}

impl Database<InMemoryGraph> {
    /// Stream a query's results through the chosen [`Format`] into
    /// `writer` using the engine's true pull cursor.
    ///
    /// Where [`Database::export_query`] materialises the full result
    /// set first (`RowArrays` projection), this variant pulls rows
    /// row-at-a-time off [`Self::stream_with_params`] and feeds each
    /// one to the encoder. Peak engine-side memory stays bounded by
    /// the encoder's internal buffer — typically tens of KiB —
    /// regardless of total row count.
    ///
    /// Specialised on [`InMemoryGraph`] because the true streaming
    /// cursor lives on that impl block.
    pub fn export_query_streaming<W: Write>(
        &self,
        query: &str,
        params: BTreeMap<String, LoraValue>,
        format: Format,
        writer: W,
    ) -> Result<ExportStats, LoraError> {
        match format {
            Format::Jsonl => self.drive_streaming_export(query, params, JsonlEncoder::new(writer)),
            Format::Json => {
                self.drive_streaming_export(query, params, JsonArrayEncoder::new(writer))
            }
            Format::Csv => self.drive_streaming_export(query, params, CsvEncoder::new(writer)),
        }
    }

    fn drive_streaming_export<E: RowEncoder>(
        &self,
        query: &str,
        params: BTreeMap<String, LoraValue>,
        mut encoder: E,
    ) -> Result<ExportStats, LoraError> {
        let mut stream = self.stream_with_params(query, params)?;
        let columns = stream.columns().to_vec();
        encoder.begin(&columns).map_err(io_err)?;
        let mut rows = 0u64;
        while let Some(row) = stream.next_row().map_err(LoraError::from_anyhow)? {
            encoder.write_row(&row).map_err(io_err)?;
            rows += 1;
        }
        encoder.finish().map_err(io_err)?;
        Ok(ExportStats { rows })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_node_import_then_export() {
        let db = Database::in_memory();

        let csv = "name:string,age:int\nalice,30\nbob,25\n";
        let mapping = RowMapping::Node {
            label: "User".into(),
            id_column: None,
            id_property: None,
            properties: vec![
                lora_io::ColumnSpec::identity("name"),
                lora_io::ColumnSpec::identity("age"),
            ],
        };
        let stats = db
            .import_rows(std::io::Cursor::new(csv), Format::Csv, &mapping, Some(10))
            .unwrap();
        assert_eq!(stats.rows, 2);
        assert_eq!(stats.batches, 1);

        let mut out = Vec::new();
        let export = db
            .export_query(
                "MATCH (u:User) RETURN u.name AS name, u.age AS age ORDER BY name",
                BTreeMap::new(),
                Format::Jsonl,
                &mut out,
            )
            .unwrap();
        assert_eq!(export.rows, 2);
        let text = std::str::from_utf8(&out).unwrap();
        let lines: Vec<_> = text.lines().collect();
        assert_eq!(lines.len(), 2);
        assert!(lines[0].contains("\"alice\""));
        assert!(lines[1].contains("\"bob\""));
    }

    /// Property names with spaces — e.g. `User Id` from a CSV header —
    /// are backtick-quoted by `RowMapping::to_cypher` and must analyze
    /// even when the graph already holds unrelated nodes. Regression
    /// for the playground bug where the second batch raised
    /// `unknown property `User Id``.
    #[test]
    fn import_handles_property_names_with_spaces_in_non_empty_graph() {
        let db = Database::in_memory();
        db.execute("CREATE (:Other {tag: 1})", None).unwrap();

        let csv = "User Id,First Name\nu1,Alice\nu2,Bob\nu3,Carol\n";
        let mapping = RowMapping::Node {
            label: "People".into(),
            id_column: None,
            id_property: None,
            properties: vec![
                lora_io::ColumnSpec::identity("User Id"),
                lora_io::ColumnSpec::identity("First Name"),
            ],
        };
        let stats = db
            .import_rows(std::io::Cursor::new(csv), Format::Csv, &mapping, Some(2))
            .expect("import with quoted property names should succeed");
        assert_eq!(stats.rows, 3);
        assert!(stats.batches >= 2, "expected at least 2 batches");
    }

    #[test]
    fn cypher_template_import() {
        let db = Database::in_memory();
        let jsonl = "{\"name\":\"alice\",\"age\":30}\n{\"name\":\"bob\",\"age\":25}\n";
        let stats = db
            .import_with_template(
                std::io::Cursor::new(jsonl),
                Format::Jsonl,
                "UNWIND $rows AS r CREATE (:Person {name: r.name, age: r.age})",
                Some(50),
            )
            .unwrap();
        assert_eq!(stats.rows, 2);

        let res = db
            .execute(
                "MATCH (p:Person) RETURN count(p) AS c",
                Some(ExecuteOptions {
                    format: ResultFormat::RowArrays,
                }),
            )
            .unwrap();
        match res {
            QueryResult::RowArrays(r) => {
                let LoraValue::Int(c) = &r.rows[0][0] else {
                    panic!("expected Int");
                };
                assert_eq!(*c, 2);
            }
            other => panic!("unexpected result {other:?}"),
        }
    }
}
