//! Builtins that panicked (aborting a release build), allocated without
//! bound or ignored the deadline on plain input. Each now returns a value
//! or fails the query with an error.

use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use lora_database::{Database, InMemoryGraph, LoraErrorCode, LoraValue};

fn db() -> Database<InMemoryGraph> {
    Database::in_memory()
}

/// The single value `query` returns.
fn value(db: &Database<InMemoryGraph>, query: &str) -> LoraValue {
    let rows = db
        .execute_rows(query)
        .unwrap_or_else(|e| panic!("{query}: {e}"));
    let value = rows[0].iter().next().map(|(_, v)| v.clone());
    value.unwrap_or_else(|| panic!("{query}: no column"))
}

/// The error `query` fails with, within `budget`.
fn error(db: &Database<InMemoryGraph>, query: &str) -> (LoraErrorCode, String) {
    let start = Instant::now();
    let err = match db.execute(query, None) {
        Ok(_) => panic!("`{query}` succeeded"),
        Err(e) => e,
    };
    assert!(
        start.elapsed() < Duration::from_secs(20),
        "{query} took {:?}",
        start.elapsed()
    );
    (err.code(), err.message().to_string())
}

#[test]
fn math_clamp_rejects_bad_bounds_and_keeps_integer_precision() {
    let db = db();
    for query in [
        "RETURN math.clamp(5, 10, 1) AS x",
        "RETURN math.clamp(5.0, 10.0, 1.0) AS x",
    ] {
        let (code, message) = error(&db, query);
        assert_eq!(code, LoraErrorCode::Validation, "{query}: {message}");
        assert!(message.contains("math.clamp"), "{message}");
    }
    assert_eq!(
        value(
            &db,
            "RETURN math.clamp(9007199254740993, 0, 9223372036854775807) AS x"
        ),
        LoraValue::Int(9007199254740993)
    );
    assert_eq!(
        value(&db, "RETURN math.clamp(5, 1, 3) AS x"),
        LoraValue::Int(3)
    );
}

#[test]
fn math_round_keeps_values_past_float_precision() {
    let db = db();
    assert_eq!(
        value(&db, "RETURN math.round(1.5, 9223372036854775807) AS x"),
        LoraValue::Float(1.5)
    );
    assert_eq!(
        value(&db, "RETURN math.round(1234.5, -9223372036854775807) AS x"),
        LoraValue::Float(0.0)
    );
    assert_eq!(
        value(&db, "RETURN math.round(1.25, 1) AS x"),
        LoraValue::Float(1.3)
    );
}

#[test]
fn sizes_are_checked_before_allocating() {
    let db = db();
    for query in [
        "RETURN string.repeat('ab', 9223372036854775807) AS x",
        "RETURN string.repeat('ab', 100000000) AS x",
        "RETURN list.repeat(1, 9223372036854775807) AS x",
        "RETURN list.repeat('x', 100000000) AS x",
        "RETURN string.pad('x', 1000000000, 'ab') AS x",
        "RETURN size(list.combinations(range(1, 26), 12)) AS x",
        "RETURN size(list.combinations(range(1, 200000), 100000)) AS x",
        "RETURN list.windows(range(1, 100000), 50000) AS x",
        "RETURN size(range(1, 100000000)) AS x",
        "WITH string.repeat('x', 1000000) AS s RETURN size(string.replace(s, 'x', s)) AS x",
        "WITH string.repeat('x', 1000000) AS s RETURN size(string.replace(s, '/x/', s)) AS x",
        "WITH string.repeat('x', 1000) AS s RETURN size(string.join(list.repeat(s, 100000), s)) AS x",
    ] {
        let (code, message) = error(&db, query);
        assert_eq!(code, LoraErrorCode::Validation, "{query}: {message}");
        assert!(message.contains("MiB"), "{query}: {message}");
    }
    // Every combination is still produced when they fit, without
    // recursion as deep as `k`.
    assert_eq!(
        value(
            &db,
            "RETURN size(list.combinations(range(1, 200000), 200000)) AS x"
        ),
        LoraValue::Int(1)
    );
    assert_eq!(
        value(
            &db,
            "RETURN size(list.combinations(range(1, 200), 199)) AS x"
        ),
        LoraValue::Int(200)
    );
    assert_eq!(
        value(&db, "RETURN list.combinations([1, 2, 3], 2) AS x"),
        LoraValue::List(vec![
            LoraValue::List(vec![LoraValue::Int(1), LoraValue::Int(2)]),
            LoraValue::List(vec![LoraValue::Int(1), LoraValue::Int(3)]),
            LoraValue::List(vec![LoraValue::Int(2), LoraValue::Int(3)]),
        ])
    );
}

#[test]
fn number_format_precision_is_bounded() {
    let db = db();
    let (code, message) = error(&db, "RETURN number.format(1.5, 70000) AS x");
    assert_eq!(code, LoraErrorCode::Validation, "{message}");
    assert_eq!(
        value(&db, "RETURN number.format(1.5, 2) AS x"),
        LoraValue::String("1.50".into())
    );
}

#[test]
fn duration_overflow_is_an_error() {
    let db = db();
    for query in [
        "RETURN duration({years: 1e300, months: 1e300}) AS x",
        "RETURN duration({months: 9223372036854775807, years: 9223372036854775807}) AS x",
        "RETURN duration({seconds: 9223372036854775807, minutes: 9223372036854775807}) AS x",
        "RETURN duration({days: 1e25}) AS x",
    ] {
        let (code, message) = error(&db, query);
        assert_eq!(code, LoraErrorCode::Validation, "{query}: {message}");
        assert!(message.contains("out of range"), "{query}: {message}");
    }
    // Opposite huge amounts that cancel exactly still fit.
    let LoraValue::Duration(_) = value(
        &db,
        "RETURN duration({months: 9223372036854775807, days: -5}) AS x",
    ) else {
        panic!("expected a duration")
    };
}

#[test]
fn wgs84_points_are_validated() {
    let db = db();
    for query in [
        "RETURN point({latitude: 1000, longitude: 0}) AS x",
        "RETURN point({latitude: 0, longitude: 1e300}) AS x",
        "RETURN point({x: 1e300 * 1e300, y: 0}) AS x",
    ] {
        let (_, message) = error(&db, query);
        assert!(
            message.contains("latitude")
                || message.contains("longitude")
                || message.contains("finite"),
            "{query}: {message}"
        );
    }
    let LoraValue::Point(_) = value(&db, "RETURN point({latitude: -90, longitude: 180}) AS x")
    else {
        panic!("expected a point")
    };
}

#[test]
fn sorting_mixed_and_nan_lists_does_not_panic() {
    let db = db();
    let LoraValue::List(sorted) = value(
        &db,
        "RETURN list.sort([3, 'a', null, 0.0 / 0.0, [1], true, 1.5, {a: 1}, -1]) AS x",
    ) else {
        panic!("expected a list")
    };
    assert_eq!(sorted.len(), 9);
    let LoraValue::Float(m) = value(&db, "RETURN list.median([3, 0.0 / 0.0, 1, 2]) AS x") else {
        panic!("expected a float")
    };
    assert!(m.is_finite() || m.is_nan());
}

#[test]
fn deeply_nested_lists_are_typed_in_linear_time() {
    let db = db();
    // Typing a nested list used to type its first item twice per level.
    let query = "RETURN type.of(reduce(acc = [1], i IN range(1, 100) | [acc])) AS x";
    let start = Instant::now();
    let LoraValue::String(_) = value(&db, query) else {
        panic!("expected a string")
    };
    assert!(start.elapsed() < Duration::from_secs(10));
}

#[test]
fn text_distance_is_bounded_and_keeps_its_results() {
    let db = db();
    assert_eq!(
        value(&db, "RETURN text.distance('ca', 'abc', 'damerau') AS x"),
        LoraValue::Int(3)
    );
    assert_eq!(
        value(
            &db,
            "RETURN text.distance('abcdef', 'abdcef', 'damerau') AS x"
        ),
        LoraValue::Int(1)
    );
    assert_eq!(
        value(
            &db,
            "RETURN text.distance('kitten', 'sitting', 'levenshtein') AS x"
        ),
        LoraValue::Int(3)
    );
    let (code, message) = error(
        &db,
        "WITH string.repeat('ab', 50000) AS s RETURN text.distance(s, s, 'damerau') AS x",
    );
    assert_eq!(code, LoraErrorCode::Validation, "{message}");
    // Long enough to take a while, short enough to be allowed: the
    // deadline stops it.
    let start = Instant::now();
    let err = db
        .execute_with_timeout(
            "WITH string.repeat('ab', 8000) AS s UNWIND range(1, 1000) AS i \
             RETURN text.distance(s + toString(i), s, 'levenshtein') AS x",
            None,
            Duration::from_millis(200),
        )
        .unwrap_err();
    assert_eq!(err.code(), LoraErrorCode::Timeout, "{err}");
    assert!(start.elapsed() < Duration::from_secs(20));
}

#[test]
fn regular_expressions_are_bounded_and_cached() {
    let db = db();
    // A pattern that compiles to more than the limit is an error, quickly.
    for query in [
        "RETURN 'a' =~ '\\\\w{1000}{1000}' AS x",
        "RETURN string.matches('a', '\\\\w{1000}{1000}') AS x",
        "RETURN string.replace('a', '/\\\\w{1000}{1000}/', 'b') AS x",
    ] {
        let (code, message) = error(&db, query);
        assert_eq!(code, LoraErrorCode::Validation, "{query}: {message}");
        assert!(message.contains("too large"), "{query}: {message}");
    }
    // An invalid pattern still makes a string function null.
    assert_eq!(
        value(&db, "RETURN string.matches('a', '(') AS x"),
        LoraValue::Null
    );
    // The string functions reuse the compiled pattern across rows.
    let start = Instant::now();
    let rows = db
        .execute_rows(
            "UNWIND range(1, 20000) AS i \
             WITH string.replace('item-' + toString(i), '/[0-9]+/', 'n') AS r \
             RETURN count(r) AS n",
        )
        .unwrap();
    assert_eq!(rows.len(), 1);
    assert!(
        start.elapsed() < Duration::from_secs(20),
        "{:?}",
        start.elapsed()
    );
}

#[test]
fn parameters_are_read_in_place() {
    let db = db();
    let mut params = BTreeMap::new();
    params.insert(
        "b".to_string(),
        LoraValue::List((0..1000).map(LoraValue::Int).collect()),
    );
    let rows = db
        .execute_rows_with_params(
            "UNWIND range(1, 2000) AS x WITH x WHERE x IN $b RETURN count(x) AS n",
            params.clone(),
        )
        .unwrap();
    assert_eq!(
        rows[0].iter().next().map(|(_, v)| v.clone()),
        Some(LoraValue::Int(999))
    );
    let rows = db
        .execute_rows_with_params("UNWIND $b AS x RETURN sum(x) AS n", params.clone())
        .unwrap();
    assert_eq!(
        rows[0].iter().next().map(|(_, v)| v.clone()),
        Some(LoraValue::Int(499500))
    );
    let rows = db
        .execute_rows_with_params("RETURN coalesce($b, []) AS b, $b[3] AS three", params)
        .unwrap();
    assert_eq!(rows.len(), 1);
}
