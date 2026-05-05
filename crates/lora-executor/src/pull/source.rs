use crate::errors::ExecResult;
use crate::value::Row;

/// Fallible pull-based row cursor.
///
/// Each call to [`RowSource::next_row`] returns the next row,
/// `Ok(None)` when the cursor is exhausted, or an error if execution
/// fails. The cursor stays in a valid state after an error — callers
/// may drop it without observing additional side effects.
pub trait RowSource {
    /// Pull the next row.
    fn next_row(&mut self) -> ExecResult<Option<Row>>;
}

/// Drain a row source into a `Vec<Row>`, propagating the first error.
pub fn drain<S: RowSource + ?Sized>(source: &mut S) -> ExecResult<Vec<Row>> {
    let mut out = Vec::new();
    while let Some(row) = source.next_row()? {
        out.push(row);
    }
    Ok(out)
}

/// Buffered cursor backed by a pre-computed `Vec<Row>`. Used both as
/// a simple "rows already collected" adapter and as the leaf fallback
/// for operators whose internals still require full materialization.
pub struct BufferedRowSource {
    iter: std::vec::IntoIter<Row>,
}

impl BufferedRowSource {
    pub fn new(rows: Vec<Row>) -> Self {
        Self {
            iter: rows.into_iter(),
        }
    }
}

impl RowSource for BufferedRowSource {
    fn next_row(&mut self) -> ExecResult<Option<Row>> {
        Ok(self.iter.next())
    }
}

/// Yields a single empty row exactly once. The bottom of every plan
/// chain that doesn't start with an explicit input.
pub struct ArgumentSource {
    yielded: bool,
}

impl ArgumentSource {
    pub fn new() -> Self {
        Self { yielded: false }
    }
}

impl Default for ArgumentSource {
    fn default() -> Self {
        Self::new()
    }
}

impl RowSource for ArgumentSource {
    fn next_row(&mut self) -> ExecResult<Option<Row>> {
        if self.yielded {
            Ok(None)
        } else {
            self.yielded = true;
            Ok(Some(Row::new()))
        }
    }
}
