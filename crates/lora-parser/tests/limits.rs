//! Query-shape limits and parse-time bounds.
//!
//! The grammar used to re-parse a nested expression once per ordered-choice
//! alternative, so parsing time doubled with each nesting level (14 nested
//! parentheses took seconds, 100 never finished), and nesting or operator
//! chains deep enough overflowed the stack. These tests pin both: every
//! nesting construct parses in linear time, and the limits in
//! `lora_ast::limits` reject deeper input with a parse error.

use std::time::{Duration, Instant};

use lora_ast::limits::{MAX_CLAUSES, MAX_EXPRESSION_DEPTH, MAX_QUERY_NESTING};
use lora_parser::parse_query;

/// Run `f` on a thread with a stack large enough for an unoptimised build
/// at the nesting limit.
fn on_big_stack<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> T {
    std::thread::Builder::new()
        .stack_size(64 << 20)
        .spawn(f)
        .unwrap()
        .join()
        .unwrap()
}

/// `open` repeated `depth` times around `inner`, closed by `close`.
fn nest(open: &str, inner: &str, close: &str, depth: usize) -> String {
    format!("{}{inner}{}", open.repeat(depth), close.repeat(depth))
}

/// Nesting constructs: (opening text, innermost text, closing text).
const CONSTRUCTS: &[(&str, &str, &str)] = &[
    ("(", "1", ")"),
    ("-(", "1", ")"),
    ("[(", "1", ")]"),
    ("(", "1", " AS INTEGER)"),
    ("[", "1", "]"),
    ("{a: ", "1", "}"),
    ("abs(", "1", ")"),
    ("coalesce(1, ", "1", ")"),
    ("cast(", "1", " AS INTEGER)"),
    ("try_cast(", "1", " AS INTEGER)"),
    ("any(x IN ", "[1]", " WHERE true)"),
    ("reduce(s = 0, x IN ", "[1]", " | s)"),
    ("reduce(s = ", "0", ", x IN [1] | s)"),
    ("[x IN ", "[1]", " | x]"),
    ("[x IN ", "[1]", " WHERE x]"),
    ("[(a)-->(b) | ", "1", "]"),
    ("[(a)-->(b) WHERE ", "true", " | 1]"),
    ("CASE WHEN true THEN ", "1", " END"),
    ("CASE ", "1", " WHEN 1 THEN 1 END"),
    ("CASE WHEN (", "true", ") THEN 1 END"),
    ("CASE WHEN -", "1", " THEN 1 END"),
    ("CASE WHEN [", "1", "] THEN 1 END"),
    ("EXISTS { MATCH (a) WHERE ", "true", " }"),
    ("COUNT { (a) WHERE ", "true", " }"),
    ("n{b: ", "1", "}"),
    ("[1][", "0", "]"),
    ("[1][", "0", "..]"),
    ("[(a {k: ", "1", "})]"),
];

const BUDGET: Duration = Duration::from_secs(2);

#[test]
fn every_nesting_construct_parses_in_linear_time() {
    on_big_stack(|| {
        // Some constructs open three levels at once. At this depth a
        // doubling per level would take millions of times the budget.
        let depth = MAX_QUERY_NESTING / 3;
        for (open, inner, close) in CONSTRUCTS {
            let valid = nest(open, inner, close, depth);
            // Also time three ways of failing: a missing closer, garbage
            // at the end and garbage in the innermost level (which fails
            // every alternative on the way out).
            let truncated = valid[..valid.len() - close.len()].to_string();
            let trailing = format!("{valid} %%");
            let innermost = nest(open, &format!("{inner} %%"), close, depth);
            for (case, body) in [
                ("valid", &valid),
                ("truncated", &truncated),
                ("trailing garbage", &trailing),
                ("inner garbage", &innermost),
            ] {
                let query = format!("WITH 1 AS n RETURN {body} AS x");
                let start = Instant::now();
                let result = parse_query(&query);
                let took = start.elapsed();
                assert!(
                    took < BUDGET,
                    "{case} `{open}...{close}` took {took:?} at depth {depth}"
                );
                if case == "valid" {
                    result.unwrap_or_else(|e| panic!("`{open}...{close}`: {e}"));
                } else {
                    assert!(result.is_err(), "{case} `{open}...{close}` parsed");
                }
            }
        }
    });
}

#[test]
fn nested_call_subqueries_parse_in_linear_time() {
    on_big_stack(|| {
        let depth = MAX_QUERY_NESTING / 2;
        for (open, inner, close) in [
            ("CALL { ", "RETURN 1 AS y", " } RETURN 1 AS y"),
            ("CALL { WITH 1 AS z ", "RETURN 1 AS y", " } RETURN 1 AS y"),
            (
                "CALL { RETURN 1 AS y UNION ",
                "RETURN 1 AS y",
                " } RETURN 1 AS y",
            ),
        ] {
            let query = nest(open, inner, close, depth);
            let start = Instant::now();
            parse_query(&query).unwrap();
            assert!(
                start.elapsed() < BUDGET,
                "`{open}` took {:?}",
                start.elapsed()
            );
        }
    });
}

#[test]
fn nesting_limit_is_enforced_before_parsing() {
    on_big_stack(|| {
        for (open, inner, close) in [
            ("[", "1", "]"),
            ("(", "1", ")"),
            ("{a: ", "1", "}"),
            ("abs(", "1", ")"),
            ("CASE WHEN true THEN ", "1", " END"),
            ("1 :: ", "INTEGER", ""),
        ] {
            // `LIST<` nests inside a single `1 :: ` cast.
            let (open, close) = if open == "1 :: " {
                ("LIST<", ">")
            } else {
                (open, close)
            };
            let prefix = if inner == "INTEGER" { "1 :: " } else { "" };
            let ok = format!(
                "RETURN {prefix}{} AS x",
                nest(open, inner, close, MAX_QUERY_NESTING - 1)
            );
            parse_query(&ok).unwrap_or_else(|e| panic!("`{open}` at the limit: {e}"));
            // Far past the limit: rejected by the scan, without recursing.
            let deep = format!("RETURN {prefix}{} AS x", nest(open, inner, close, 100_000));
            let err = parse_query(&deep).unwrap_err().to_string();
            assert!(err.contains("levels deep"), "`{open}`: {err}");
        }
    });
}

#[test]
fn nesting_scan_skips_strings_comments_and_names() {
    let brackets = "[".repeat(MAX_QUERY_NESTING * 2);
    for query in [
        format!("RETURN '{brackets}' AS x"),
        format!("RETURN \"{brackets}\" AS x"),
        format!("RETURN 1 AS `{brackets}`"),
        format!("RETURN 1 AS x // {brackets}"),
        format!("RETURN /* {brackets} */ 1 AS x"),
        format!("RETURN 'it\\'s {brackets}' AS x"),
    ] {
        parse_query(&query).unwrap_or_else(|e| panic!("{query}: {e}"));
    }
}

#[test]
fn end_used_as_a_name_does_not_hide_case_nesting() {
    // `end` as a variable or property must not count as closing a CASE,
    // or nested CASEs would slip past the limit.
    for level in [
        "CASE WHEN end THEN ",
        "CASE WHEN n.end THEN ",
        "CASE WHEN {end: 1} THEN ",
    ] {
        let query = format!(
            "RETURN {}1{} AS x",
            level.repeat(10_000),
            " END".repeat(10_000)
        );
        let err = parse_query(&query).unwrap_err().to_string();
        assert!(err.contains("levels deep"), "{level}: {err}");
    }
    // Sequential CASEs close properly and never accumulate.
    let many = vec!["CASE WHEN n.end THEN 1 ELSE end END"; 500].join(", ");
    parse_query(&format!("WITH 1 AS n, 2 AS end RETURN {many}")).unwrap();
}

#[test]
fn operator_chains_are_bounded() {
    on_big_stack(|| {
        let infix = [
            (" + ", "1"),
            (" AND ", "true"),
            (" = ", "1"),
            (" ^ ", "1"),
            (" IN ", "[1]"),
        ];
        for (op, operand) in infix {
            let chain = |n: usize| format!("{operand}{}", format!("{op}{operand}").repeat(n));
            parse_query(&format!("RETURN {} AS r", chain(MAX_EXPRESSION_DEPTH)))
                .unwrap_or_else(|e| panic!("`{op}` chain at the limit: {e}"));
            let err = parse_query(&format!("RETURN {} AS r", chain(10_000)))
                .unwrap_err()
                .to_string();
            assert!(err.contains("operators deep"), "`{op}`: {err}");
        }
        for prefix in ["NOT ", "- "] {
            let chain = |n: usize| format!("{}1", prefix.repeat(n));
            parse_query(&format!("RETURN {} AS r", chain(MAX_EXPRESSION_DEPTH)))
                .unwrap_or_else(|e| panic!("`{prefix}` chain at the limit: {e}"));
            let err = parse_query(&format!("RETURN {} AS r", chain(10_000)))
                .unwrap_err()
                .to_string();
            assert!(err.contains("operators deep"), "`{prefix}`: {err}");
        }
        // Postfix chains.
        for postfix in [".a", "[0]", "::INTEGER"] {
            let past = format!(
                "RETURN [1]{} AS r",
                postfix.repeat(MAX_EXPRESSION_DEPTH + 1)
            );
            let err = parse_query(&past).unwrap_err().to_string();
            assert!(err.contains("operators deep"), "`{postfix}`: {err}");
        }
        // Depth adds up through brackets.
        let half = vec!["1"; MAX_EXPRESSION_DEPTH / 2 + 1].join(" + ");
        let err = parse_query(&format!("RETURN {half} + ({half}) AS r"))
            .unwrap_err()
            .to_string();
        assert!(err.contains("operators deep"), "{err}");
        // Wide is fine: long lists and many projections are not deep.
        let wide = vec!["1 + 1"; 10_000].join(", ");
        parse_query(&format!("RETURN [{wide}] AS r")).unwrap();
    });
}

#[test]
fn clause_count_is_bounded() {
    on_big_stack(|| {
        let at_limit = format!(
            "WITH 1 AS a {}RETURN a",
            "WITH a WHERE a = 1 ".repeat(MAX_CLAUSES - 2)
        );
        parse_query(&at_limit).unwrap();
        let past = format!("WITH 1 AS a {}RETURN a", "WITH a ".repeat(10_000));
        let err = parse_query(&past).unwrap_err().to_string();
        assert!(err.contains("clauses"), "{err}");
        // CALL bodies count towards their outer query.
        let inner = format!("CALL {{ {}RETURN 1 AS b }} ", "WITH 1 AS c ".repeat(200));
        let outer = format!("{inner}{}RETURN 1 AS d", "WITH 1 AS e ".repeat(100));
        let err = parse_query(&outer).unwrap_err().to_string();
        assert!(err.contains("clauses"), "{err}");
        // UNION branches are counted separately.
        let union = vec!["MATCH (n) WITH n RETURN 1 AS a"; MAX_CLAUSES].join(" UNION ");
        parse_query(&union).unwrap();
    });
}

#[test]
fn reshaped_grammar_keeps_its_meaning() {
    // `(x AS T)` casts; `(x)` does not.
    for query in [
        "RETURN (1 AS STRING) AS x",
        "RETURN (1) AS x",
        "RETURN cast(1 AS STRING) AS x",
        "RETURN [1, 2][0] AS x",
        "RETURN [1, 2][..1] AS x",
        "RETURN [1, 2][0..] AS x",
        "RETURN [1, 2][0..1] AS x",
        "RETURN [x IN [1, 2] | x] AS x",
        "MATCH (n) RETURN [(n)-->(m) | m] AS x",
        "MATCH (n) RETURN [(v:P {k: 1}) WHERE v.ok | 1] AS x",
        "MATCH (n) RETURN [((n)<-[:R]-(m)) | m] AS x",
        "RETURN [(x IN [1, 2]), 3] AS x",
        "RETURN CASE 1 WHEN 1 THEN 2 END AS x",
        "RETURN CASE WHEN true THEN 2 END AS x",
        "MATCH (n) WITH n MATCH (m) CREATE (n)-[:R]->(m) WITH n SET n.a = 1 RETURN n",
        "MATCH (n) CALL { WITH n CREATE (m) }",
        "CALL db.labels()",
        "CALL db.labels() YIELD label",
        "CALL db.labels() YIELD label RETURN label",
        "CREATE (n)",
    ] {
        parse_query(query).unwrap_or_else(|e| panic!("{query}: {e}"));
    }
    for (query, message) in [
        ("RETURN [x IN [1, 2], 3] AS x", "parentheses"),
        ("RETURN [1][] AS x", "index"),
        ("MATCH (n) RETURN n MATCH (m) RETURN m", "last clause"),
        ("CREATE (n) MATCH (m) RETURN m", "WITH"),
        ("MATCH (n) WITH n", "must end"),
        ("MATCH (n)", "must end"),
    ] {
        let err = parse_query(query).unwrap_err().to_string();
        assert!(err.contains(message), "{query}: {err}");
    }
    // Not functions: `cast(x)` without a type and `any(...)` without WHERE.
    for query in ["RETURN cast(1) AS x", "RETURN any(x IN [1]) AS x"] {
        assert!(parse_query(query).is_err(), "{query} parsed");
    }
}
