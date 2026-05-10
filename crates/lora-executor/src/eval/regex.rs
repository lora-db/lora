//! Regex predicate helpers for expression evaluation.
//!
//! Cypher's `=~` operator is a full-string match. The executor evaluates
//! predicates per row, so literal regex filters would otherwise recompile the
//! same anchored pattern once for every candidate row.

use std::cell::RefCell;
use std::collections::HashMap;

const CACHE_MAX: usize = 128;

thread_local! {
    static CACHE: RefCell<HashMap<String, ::regex::Regex>> = RefCell::new(HashMap::new());
}

pub(super) fn full_match(value: &str, pattern: &str) -> Option<bool> {
    let anchored = format!("^(?:{pattern})$");

    CACHE.with(|cache| {
        {
            let cached = cache.borrow();
            if let Some(regex) = cached.get(&anchored) {
                return Some(regex.is_match(value));
            }
        }

        let regex = ::regex::Regex::new(&anchored).ok()?;
        let matched = regex.is_match(value);

        let mut cached = cache.borrow_mut();
        if cached.len() >= CACHE_MAX {
            cached.clear();
        }
        cached.insert(anchored, regex);

        Some(matched)
    })
}
