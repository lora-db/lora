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

/// Checks the stack inside a deep pull pipeline. Each source pulls from
/// the one below it, so pulling a row recurses as deep as the pipeline
/// (a long pattern plans one expand per hop). Every
/// [`lora_ast::stack::CHECK_INTERVAL`]th source of a chain is wrapped, so
/// the rows of a shallow pipeline never pass through one, and a deep one
/// continues on a fresh stack segment when the thread's runs low.
pub(crate) struct StackSource<'a> {
    /// Taken out only by `drop`, which frees the rest of the chain under a
    /// stack guard as well: freeing a source frees its upstream first.
    inner: std::mem::ManuallyDrop<Box<dyn RowSource + 'a>>,
}

impl<'a> StackSource<'a> {
    /// Wrap `inner`, built at `depth`, when a check falls there. The
    /// sources between two checks, and the expressions they evaluate, stay
    /// well inside a stack guard's red zone.
    pub(crate) fn wrap(inner: Box<dyn RowSource + 'a>, depth: usize) -> Box<dyn RowSource + 'a> {
        if lora_ast::stack::check_due(depth) {
            Box::new(Self {
                inner: std::mem::ManuallyDrop::new(inner),
            })
        } else {
            inner
        }
    }
}

impl RowSource for StackSource<'_> {
    fn next_row(&mut self) -> ExecResult<Option<Row>> {
        lora_ast::stack::guard(|| self.inner.next_row())
    }
}

impl Drop for StackSource<'_> {
    fn drop(&mut self) {
        // SAFETY: `inner` is not used again after this.
        let inner = unsafe { std::mem::ManuallyDrop::take(&mut self.inner) };
        lora_ast::stack::guard(|| drop(inner));
    }
}

/// Enforces a query deadline inside a pull pipeline. Wrapped around every
/// source when the query has a deadline, so even a loop that never yields
/// a row (a filter rejecting a cartesian product, say) keeps checking:
/// each pull from upstream passes through a wrapper. The clock is read
/// every 64 pulls to keep the overhead negligible.
pub(crate) struct DeadlineSource<'a> {
    inner: Box<dyn RowSource + 'a>,
    deadline: web_time::Instant,
    tick: u32,
}

impl<'a> DeadlineSource<'a> {
    pub(crate) fn wrap(
        inner: Box<dyn RowSource + 'a>,
        deadline: Option<web_time::Instant>,
    ) -> Box<dyn RowSource + 'a> {
        match deadline {
            Some(deadline) => Box::new(Self {
                inner,
                deadline,
                tick: 0,
            }),
            None => inner,
        }
    }
}

impl RowSource for DeadlineSource<'_> {
    fn next_row(&mut self) -> ExecResult<Option<Row>> {
        self.tick = self.tick.wrapping_add(1);
        if self.tick.is_multiple_of(64) && crate::cancel::deadline_reached(self.deadline) {
            return Err(crate::errors::ExecutorError::QueryTimeout);
        }
        let row = self.inner.next_row()?;
        // An expression evaluated for this row may have stopped early on
        // the deadline (see `cancel::eval_deadline_hit`); its value is
        // incomplete, so the row must not escape.
        if crate::cancel::eval_tripped() {
            return Err(crate::errors::ExecutorError::QueryTimeout);
        }
        Ok(row)
    }
}
