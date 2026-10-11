//! Regex predicate helpers for expression evaluation.
//!
//! Cypher's `=~` operator is a full-string match. The executor evaluates
//! predicates per row, so literal regex filters would otherwise recompile the
//! same anchored pattern once for every candidate row.

use std::cell::RefCell;
use std::collections::HashMap;

const CACHE_MAX: usize = 128;

thread_local! {
    /// Compiled patterns, and the message of those that do not compile: a
    /// pattern that is too large takes as long to reject as to compile.
    static CACHE: RefCell<HashMap<String, Result<::regex::Regex, String>>> =
        RefCell::new(HashMap::new());
}

pub(super) fn full_match(value: &str, pattern: &str) -> Result<bool, String> {
    let anchored = format!("^(?:{pattern})$");

    CACHE.with(|cache| {
        if let Some(compiled) = cache.borrow().get(&anchored) {
            return compiled
                .as_ref()
                .map(|r| r.is_match(value))
                .map_err(String::clone);
        }

        let compiled = ::regex::Regex::new(&anchored).map_err(|err| describe(pattern, &err));
        let result = compiled
            .as_ref()
            .map(|r| r.is_match(value))
            .map_err(String::clone);

        let mut cached = cache.borrow_mut();
        if cached.len() >= CACHE_MAX {
            cached.clear();
        }
        cached.insert(anchored, compiled);

        result
    })
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
