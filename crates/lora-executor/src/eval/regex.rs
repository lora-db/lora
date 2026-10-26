//! Regex predicate helpers for expression evaluation.
//!
//! Cypher's `=~` operator is a full-string match. The executor evaluates
//! predicates per row, so literal regex filters would otherwise recompile the
//! same anchored pattern once for every candidate row.

use std::cell::RefCell;
use std::collections::HashMap;

/// Patterns kept compiled per thread. Each is bounded by
/// [`COMPILED_SIZE_LIMIT`] and [`DFA_SIZE_LIMIT`], so the cache holds at most
/// a few hundred MiB even when every entry is as large as allowed (it held
/// 1.4 GB after 127 patterns like `\w{200}`); in practice entries are a few
/// KiB.
const CACHE_MAX: usize = 64;

/// Longest pattern accepted, in bytes.
const MAX_PATTERN_BYTES: usize = 16 << 10;

/// Largest compiled program a pattern may produce (the regex crate's
/// default is 10 MiB); larger ones fail to compile with an error.
const COMPILED_SIZE_LIMIT: usize = 2 << 20;

/// Largest lazy-DFA cache one pattern may grow while matching.
const DFA_SIZE_LIMIT: usize = 2 << 20;

/// Why a pattern did not compile.
#[derive(Debug, Clone)]
pub(crate) struct RegexError {
    pub(crate) message: String,
    /// The pattern is valid but too large (or too long) to compile: an
    /// error even where an invalid pattern yields null.
    pub(crate) too_large: bool,
}

thread_local! {
    /// Compiled patterns, and the error of those that do not compile: a
    /// pattern that is too large takes as long to reject as to compile.
    static CACHE: RefCell<HashMap<String, Result<::regex::Regex, RegexError>>> =
        RefCell::new(HashMap::new());
}

/// `pattern` compiled, from this thread's cache when it was compiled
/// before. Cloning a compiled regex is a reference-count bump.
pub(crate) fn compile(pattern: &str) -> Result<::regex::Regex, RegexError> {
    compile_as(pattern, pattern)
}

/// [`compile`], naming `shown` (the pattern as the user wrote it) in
/// errors.
fn compile_as(pattern: &str, shown: &str) -> Result<::regex::Regex, RegexError> {
    CACHE.with(|cache| {
        if let Some(compiled) = cache.borrow().get(pattern) {
            return compiled.clone();
        }
        let compiled = build(pattern, shown);
        let mut cached = cache.borrow_mut();
        if cached.len() >= CACHE_MAX {
            cached.clear();
        }
        cached.insert(pattern.to_string(), compiled.clone());
        compiled
    })
}

fn build(pattern: &str, shown: &str) -> Result<::regex::Regex, RegexError> {
    if pattern.len() > MAX_PATTERN_BYTES {
        return Err(RegexError {
            message: format!(
                "regular expression is too long ({} bytes; the limit is {MAX_PATTERN_BYTES})",
                shown.len()
            ),
            too_large: true,
        });
    }
    ::regex::RegexBuilder::new(pattern)
        .size_limit(COMPILED_SIZE_LIMIT)
        .dfa_size_limit(DFA_SIZE_LIMIT)
        .build()
        .map_err(|err| RegexError {
            message: describe(shown, &err),
            too_large: matches!(err, ::regex::Error::CompiledTooBig(_)),
        })
}

pub(super) fn full_match(value: &str, pattern: &str) -> Result<bool, String> {
    let anchored = format!("^(?:{pattern})$");
    compile_as(&anchored, pattern)
        .map(|re| re.is_match(value))
        .map_err(|err| err.message)
}

fn describe(pattern: &str, err: &::regex::Error) -> String {
    let shown: String = pattern.chars().take(80).collect();
    let shown = if shown.len() < pattern.len() {
        format!("{shown}...")
    } else {
        shown
    };
    match err {
        ::regex::Error::CompiledTooBig(limit) => {
            format!("regular expression `{shown}` is too large to compile (limit {limit} bytes)")
        }
        // The syntax error's last line names the problem; the lines before
        // it draw the pattern with a caret.
        other => {
            let text = other.to_string();
            let reason = text
                .lines()
                .last()
                .unwrap_or("")
                .trim()
                .trim_start_matches("error: ");
            format!("invalid regular expression `{shown}`: {reason}")
        }
    }
}
