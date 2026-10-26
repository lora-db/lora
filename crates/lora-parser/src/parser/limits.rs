//! Shape limits enforced while parsing (see [`lora_ast::limits`]).
//!
//! The grammar and the lowering recurse once per nesting level, so query
//! text that nests deeper than the stack allows must be refused before
//! pest sees it: [`check_nesting`] does that with one linear scan. Operator
//! chains (`1 + 1 + ...`, `NOT NOT ...`) parse iteratively but lower into
//! trees as deep as the chain is long, which every later stage walks
//! recursively; [`DepthGuard`] bounds those while lowering, before a deep
//! tree exists.

use std::cell::Cell;

use lora_ast::limits::{MAX_CLAUSES, MAX_EXPRESSION_DEPTH, MAX_QUERY_NESTING};

use crate::errors::ParseError;

/// Refuse `input` when its brackets, parentheses, braces, `CASE`
/// expressions or `LIST<...>` types nest deeper than
/// [`MAX_QUERY_NESTING`]. String literals, escaped names and comments are
/// skipped. The count errs high, never low: a closer only lowers it when
/// it can close something.
pub(super) fn check_nesting(input: &str) -> Result<(), ParseError> {
    let bytes = input.as_bytes();
    let mut depth = 0usize;
    // Open `LIST<` types, closed by `>`.
    let mut list_types = 0usize;
    // The last significant byte and, when it ended a word, that word.
    let mut prev_byte = 0u8;
    let mut prev_word: &[u8] = b"";
    let mut i = 0;

    while i < bytes.len() {
        let b = bytes[i];
        match b {
            b'\'' | b'"' => {
                i = skip_string(bytes, i);
                prev_byte = b;
                prev_word = b"";
                continue;
            }
            b'`' => {
                i = match memchr(b'`', &bytes[i + 1..]) {
                    Some(at) => i + 1 + at + 1,
                    None => bytes.len(),
                };
                prev_byte = b'`';
                prev_word = b"";
                continue;
            }
            b'/' if bytes.get(i + 1) == Some(&b'/') => {
                i = match memchr(b'\n', &bytes[i..]) {
                    Some(at) => i + at + 1,
                    None => bytes.len(),
                };
                continue;
            }
            b'/' if bytes.get(i + 1) == Some(&b'*') => {
                i = match find(&bytes[i + 2..], b"*/") {
                    Some(at) => i + 2 + at + 2,
                    None => bytes.len(),
                };
                continue;
            }
            b'(' | b'[' | b'{' => {
                depth += 1;
                if depth > MAX_QUERY_NESTING {
                    return Err(too_deep(i));
                }
            }
            b')' | b']' | b'}' => depth = depth.saturating_sub(1),
            b'<' if prev_word.eq_ignore_ascii_case(b"LIST") => {
                list_types += 1;
                depth += 1;
                if depth > MAX_QUERY_NESTING {
                    return Err(too_deep(i));
                }
            }
            b'>' if list_types > 0 => {
                list_types -= 1;
                depth = depth.saturating_sub(1);
            }
            b if b.is_ascii_alphabetic() || b == b'_' => {
                let start = i;
                while i < bytes.len() && is_ident_part(bytes[i]) {
                    i += 1;
                }
                let word = &bytes[start..i];
                // A name right after `.` or `:` is a property key or a
                // label, never a keyword.
                if prev_byte != b'.' && prev_byte != b':' {
                    if word.eq_ignore_ascii_case(b"CASE") {
                        depth += 1;
                        if depth > MAX_QUERY_NESTING {
                            return Err(too_deep(start));
                        }
                    } else if word.eq_ignore_ascii_case(b"END") && closes_case(prev_byte, prev_word)
                    {
                        depth = depth.saturating_sub(1);
                    }
                }
                prev_byte = bytes[i - 1];
                prev_word = word;
                continue;
            }
            b if b.is_ascii_whitespace() => {
                i += 1;
                continue;
            }
            _ => {}
        }
        prev_byte = b;
        prev_word = b"";
        i += 1;
    }
    Ok(())
}

/// Whether an `END` after `prev_byte` / `prev_word` can close a `CASE`:
/// only when it follows the end of an expression. Where an expression
/// would start instead (after an operator, an opening bracket or a keyword
/// such as `THEN`), `end` is a variable name. Erring towards "no" keeps
/// the depth count high, never low.
fn closes_case(prev_byte: u8, prev_word: &[u8]) -> bool {
    const EXPRESSION_STARTS_AFTER: &[&[u8]] = &[
        b"CASE",
        b"WHEN",
        b"THEN",
        b"ELSE",
        b"AS",
        b"AND",
        b"OR",
        b"XOR",
        b"NOT",
        b"IN",
        b"IS",
        b"CONTAINS",
        b"WITH",
        b"RETURN",
        b"WHERE",
        b"BY",
        b"DISTINCT",
        b"UNWIND",
        b"SKIP",
        b"LIMIT",
        b"SET",
        b"DELETE",
        b"YIELD",
    ];
    if prev_word.is_empty() {
        return prev_byte.is_ascii_digit()
            || matches!(prev_byte, b')' | b']' | b'}' | b'\'' | b'"' | b'`' | b'*');
    }
    !EXPRESSION_STARTS_AFTER
        .iter()
        .any(|kw| prev_word.eq_ignore_ascii_case(kw))
}

fn is_ident_part(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_' || b == b'$'
}

/// The index just past the string literal opening at `start`.
fn skip_string(bytes: &[u8], start: usize) -> usize {
    let quote = bytes[start];
    let mut i = start + 1;
    while i < bytes.len() {
        match bytes[i] {
            b'\\' => i += 2,
            b if b == quote => return i + 1,
            _ => i += 1,
        }
    }
    bytes.len()
}

fn memchr(needle: u8, haystack: &[u8]) -> Option<usize> {
    haystack.iter().position(|&b| b == needle)
}

fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack.windows(needle.len()).position(|w| w == needle)
}

fn too_deep(at: usize) -> ParseError {
    ParseError::new(
        format!(
            "query nests brackets, braces, CASE expressions or LIST types more than \
             {MAX_QUERY_NESTING} levels deep"
        ),
        at,
        at + 1,
    )
}

thread_local! {
    /// Expression-tree depth committed by the operator chains that enclose
    /// the expression being lowered.
    static EXPRESSION_DEPTH: Cell<usize> = const { Cell::new(0) };
    /// Clauses committed by the query bodies that enclose the clause being
    /// lowered (a `CALL { ... }` body runs inside its outer query).
    static CLAUSES: Cell<usize> = const { Cell::new(0) };
}

/// Holds `cost` levels of the expression-depth or clause budget while the
/// operands under it are lowered; releases them on drop.
pub(super) struct DepthGuard {
    counter: &'static std::thread::LocalKey<Cell<usize>>,
    cost: usize,
}

impl DepthGuard {
    /// Reserve `cost` levels of expression depth for an operator chain
    /// spanning `start..end`.
    pub(super) fn expression(cost: usize, start: usize, end: usize) -> Result<Self, ParseError> {
        Self::enter(&EXPRESSION_DEPTH, cost, MAX_EXPRESSION_DEPTH, || {
            ParseError::new(
                format!(
                    "expression nests more than {MAX_EXPRESSION_DEPTH} operators deep; \
                     split it up, for example with WITH or a list function"
                ),
                start,
                end,
            )
        })
    }

    /// Reserve `cost` clauses for a query body spanning `start..end`.
    pub(super) fn clauses(cost: usize, start: usize, end: usize) -> Result<Self, ParseError> {
        Self::enter(&CLAUSES, cost, MAX_CLAUSES, || {
            ParseError::new(
                format!("query has more than {MAX_CLAUSES} clauses"),
                start,
                end,
            )
        })
    }

    fn enter(
        counter: &'static std::thread::LocalKey<Cell<usize>>,
        cost: usize,
        max: usize,
        error: impl FnOnce() -> ParseError,
    ) -> Result<Self, ParseError> {
        let used = counter.with(Cell::get);
        if cost > max.saturating_sub(used) {
            return Err(error());
        }
        counter.with(|c| c.set(used + cost));
        Ok(Self { counter, cost })
    }
}

impl Drop for DepthGuard {
    fn drop(&mut self) {
        let cost = self.cost;
        self.counter.with(|c| c.set(c.get() - cost));
    }
}

/// Clears both budgets; a parse starts from zero even if an earlier one on
/// this thread unwound mid-lowering.
pub(super) fn reset_budgets() {
    EXPRESSION_DEPTH.with(|c| c.set(0));
    CLAUSES.with(|c| c.set(0));
}
