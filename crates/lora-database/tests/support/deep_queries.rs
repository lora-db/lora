//! Queries and values at every shape limit of `lora_ast::limits`, and
//! patterns far longer than any query needs, shared by
//! `tests/small_stack.rs` and `examples/stack_probe.rs`.

use std::collections::BTreeMap;

use lora_ast::limits::{MAX_CLAUSES, MAX_EXPRESSION_DEPTH, MAX_QUERY_NESTING, MAX_VALUE_NESTING};
use lora_database::{parse_query, Database, InMemoryGraph, LoraValue, TransactionMode};

pub struct DeepQuery {
    pub name: String,
    pub query: String,
    pub params: BTreeMap<String, LoraValue>,
    /// Set for a parameter `$p` nested this deep, past the limit: built
    /// fresh for each call, as cloning or dropping it recursively would
    /// overflow the test's own stack.
    pub too_deep: Option<usize>,
}

impl DeepQuery {
    /// One copy of the parameters for each call [`Self::run`] makes. Copy
    /// them on a big stack: a caller cloning a deeply nested value recurses
    /// as deep as the value.
    pub fn param_copies(&self) -> Vec<BTreeMap<String, LoraValue>> {
        (0..5)
            .map(|_| match self.too_deep {
                Some(depth) => BTreeMap::from([("p".to_string(), nested_list(depth))]),
                None => self.params.clone(),
            })
            .collect()
    }

    /// Run the query through every entry point: execute, stream, explain,
    /// profile and a transaction, with the copies from
    /// [`Self::param_copies`]. Errors are fine (some cases are past a limit
    /// on purpose).
    pub fn run(&self, db: &Database<InMemoryGraph>, params: Vec<BTreeMap<String, LoraValue>>) {
        let mut params = params.into_iter();
        let mut next = || params.next().unwrap_or_default();
        let _ = db.execute_with_params(&self.query, None, next());
        if let Ok(stream) = db.stream_with_params(&self.query, next()) {
            for row in stream {
                drop(row);
            }
        }
        let _ = db.explain(&self.query, Some(next()));
        let _ = db.profile(&self.query, Some(next()));
        if let Ok(mut tx) = db.begin_transaction(TransactionMode::ReadWrite) {
            let _ = tx.execute_with_params(&self.query, None, next());
            let _ = tx.rollback();
        }
    }
}

/// `open` repeated `depth` times around `inner`, closed by `close`.
fn nest(open: &str, inner: &str, close: &str, depth: usize) -> String {
    format!("{}{inner}{}", open.repeat(depth), close.repeat(depth))
}

/// A list nested `depth` levels deep around `1`.
pub fn nested_list(depth: usize) -> LoraValue {
    let mut v = LoraValue::Int(1);
    for _ in 0..depth {
        v = LoraValue::List(vec![v]);
    }
    v
}

/// A map nested `depth` levels deep around `1`.
pub fn nested_map(depth: usize) -> LoraValue {
    let mut v = LoraValue::Int(1);
    for _ in 0..depth {
        v = LoraValue::Map(BTreeMap::from([("a".to_string(), v)]));
    }
    v
}

/// Largest `n` in `1..=max` for which `make(n)` parses (parse errors grow
/// monotonically with `n`). Runs on the caller's stack, so call it on a
/// big one.
fn deepest(max: usize, make: impl Fn(usize) -> String) -> Option<String> {
    let (mut lo, mut hi) = (0usize, max);
    while lo < hi {
        let mid = (lo + hi).div_ceil(2);
        if parse_query(&make(mid)).is_ok() {
            lo = mid;
        } else {
            hi = mid - 1;
        }
    }
    (lo > 0).then(|| make(lo))
}

/// Hops of the long patterns, and length of the `:C` chain they walk.
const HOPS: usize = 2_000;

/// Setup statements for the graph the queries run on: two `:N` nodes and
/// a chain `(:C {i: 0})-[:NEXT]->(:C {i: 1})-[:NEXT]->...` of `HOPS + 1`
/// nodes.
pub fn setup() -> Vec<String> {
    vec![
        "CREATE (:N {i: 1})-[:R {w: 1}]->(:N {i: 2})".to_string(),
        "CREATE INDEX FOR (c:C) ON (c.i)".to_string(),
        format!("UNWIND range(0, {HOPS}) AS i CREATE (:C {{i: i}})"),
        format!(
            "UNWIND range(1, {HOPS}) AS i MATCH (a:C {{i: i - 1}}) MATCH (b:C {{i: i}}) \
             CREATE (a)-[:NEXT]->(b)"
        ),
    ]
}

/// Every nesting construct the grammar has, nested as deep as the limit
/// allows with the longest operator chain that still fits inside, plus
/// clause chains, nested subqueries and values at the nesting limit.
pub fn deep_queries() -> Vec<DeepQuery> {
    let mut out = Vec::new();
    let mut push = |name: String, query: String, params: BTreeMap<String, LoraValue>| {
        out.push(DeepQuery {
            name,
            query,
            params,
            too_deep: None,
        });
    };

    // (opening text, innermost operand, operator chaining it, closing text)
    let constructs: &[(&str, &str, &str, &str)] = &[
        ("(", "1", " + ", ")"),
        ("-(", "1", " + ", ")"),
        ("NOT (", "true", " AND ", ")"),
        ("[(", "1", " + ", ")]"),
        ("(", "1", " + ", " AS INTEGER)"),
        ("[", "1", " + ", "]"),
        ("{a: ", "1", " + ", "}"),
        ("abs(", "1", " + ", ")"),
        ("coalesce(1, ", "1", " + ", ")"),
        ("cast(", "1", " + ", " AS INTEGER)"),
        ("try_cast(", "1", " + ", " AS INTEGER)"),
        ("any(x IN ", "[1]", " + ", " WHERE true)"),
        ("reduce(s = 0, x IN ", "[1]", " + ", " | s)"),
        ("reduce(s = ", "0", " + ", ", x IN [1] | s)"),
        ("[x IN ", "[1]", " + ", " | x]"),
        ("[x IN ", "[1]", " + ", " WHERE true]"),
        ("[(a)-->(b) | ", "1", " + ", "]"),
        ("[(a)-->(b) WHERE ", "true", " AND ", " | 1]"),
        ("CASE WHEN true THEN ", "1", " + ", " END"),
        ("CASE ", "1", " + ", " WHEN 1 THEN 1 END"),
        ("CASE WHEN (", "true", " AND ", ") THEN 1 END"),
        ("EXISTS { MATCH (a)-->(c) WHERE ", "true", " AND ", " }"),
        ("COUNT { (a)-->(c) WHERE ", "true", " AND ", " } > 0"),
        ("a{b: ", "1", " + ", "}"),
        ("[1][", "0", " + ", "]"),
        ("[1][", "0", " + ", "..]"),
        ("[(a {i: ", "1", " + ", "})-->(b) | 1]"),
    ];
    for (open, operand, op, close) in constructs {
        let query = |depth: usize, chain: usize| {
            let inner = format!("{operand}{}", format!("{op}{operand}").repeat(chain));
            format!(
                "MATCH (a:N {{i: 1}}) RETURN {} AS x",
                nest(open, &inner, close, depth)
            )
        };
        if let Err(e) = parse_query(&query(1, 0)) {
            panic!("`{open}...{close}` does not parse: {e}");
        }
        let mut depth = 1;
        while depth < MAX_QUERY_NESTING && parse_query(&query(depth + 1, 0)).is_ok() {
            depth += 1;
        }
        let deep = deepest(MAX_EXPRESSION_DEPTH, |c| query(depth, c)).unwrap_or(query(depth, 0));
        push(format!("nested `{open}`"), deep, BTreeMap::new());
    }

    // Prefix and infix chains alone, at the operator-depth limit.
    for (prefix, operand) in [("NOT ", "true"), ("- ", "1")] {
        let q = deepest(MAX_EXPRESSION_DEPTH, |n| {
            format!("RETURN {}{operand} AS r", prefix.repeat(n))
        })
        .unwrap();
        push(format!("prefix `{prefix}`"), q, BTreeMap::new());
    }
    for (op, operand) in [
        (" + ", "1"),
        (" AND ", "true"),
        (" OR ", "false"),
        (" = ", "1"),
        (" < ", "1"),
        (" ^ ", "1"),
        (" IN ", "[1]"),
        (" + ", "'s'"),
        (" + ", "[1]"),
    ] {
        let q = deepest(MAX_EXPRESSION_DEPTH + 1, |n| {
            format!(
                "RETURN {operand}{} AS r",
                format!("{op}{operand}").repeat(n)
            )
        })
        .unwrap();
        push(format!("chain `{op}{operand}`"), q, BTreeMap::new());
    }
    for postfix in [".a", "[0]", "::INTEGER"] {
        let base = if postfix == ".a" { "{a: 1}" } else { "[1]" };
        let q = deepest(MAX_EXPRESSION_DEPTH + 1, |n| {
            format!("RETURN {base}{} AS r", postfix.repeat(n))
        })
        .unwrap();
        push(format!("postfix `{postfix}`"), q, BTreeMap::new());
    }
    // A chain in a WHERE filter, run once per row.
    let q = deepest(MAX_EXPRESSION_DEPTH + 1, |n| {
        format!(
            "MATCH (a:N) WHERE a.i = 1{} RETURN a.i AS i",
            " OR a.i = 1".repeat(n)
        )
    })
    .unwrap();
    push("filter chain".into(), q, BTreeMap::new());

    // Clause chains and nested subqueries.
    let q = deepest(MAX_CLAUSES, |n| {
        format!(
            "MATCH (a:N) WITH a {}RETURN a.i AS i",
            "WITH a WHERE a.i >= 1 ".repeat(n)
        )
    })
    .unwrap();
    push("clause chain".into(), q, BTreeMap::new());
    let q = deepest(MAX_CLAUSES, |n| {
        format!(
            "UNWIND [1] AS a {}RETURN a",
            "WITH a + 1 AS a ORDER BY a ".repeat(n)
        )
    })
    .unwrap();
    push("ordered clause chain".into(), q, BTreeMap::new());
    let q = deepest(MAX_CLAUSES, |n| {
        format!(
            "{}RETURN 1 AS z",
            "MATCH (a:N) OPTIONAL MATCH (a)-->(b) ".repeat(n)
        )
    })
    .unwrap();
    push("match chain".into(), q, BTreeMap::new());
    for (open, inner, close) in [
        ("CALL { ", "RETURN 1 AS y", " } RETURN y"),
        ("CALL { WITH 1 AS z ", "RETURN 1 AS y", " } RETURN y"),
        (
            "CALL { RETURN 1 AS y UNION ",
            "RETURN 1 AS y",
            " } RETURN y",
        ),
        (
            "MATCH (a:N) CALL { WITH a ",
            "RETURN a.i AS y",
            " } RETURN y",
        ),
    ] {
        let q = deepest(MAX_QUERY_NESTING, |d| nest(open, inner, close, d)).unwrap();
        push(format!("nested `{open}`"), q, BTreeMap::new());
    }
    for (open, inner, close) in [
        ("EXISTS { MATCH (a)-->(b) WHERE ", "true", " }"),
        ("COUNT { MATCH (a)-->(b) WHERE ", "true", " } > 0"),
    ] {
        let q = deepest(MAX_QUERY_NESTING, |d| {
            format!(
                "MATCH (a:N) WHERE {} RETURN 1 AS y",
                nest(open, inner, close, d)
            )
        })
        .unwrap_or_else(|| panic!("`{open}` does not parse"));
        push(format!("nested `{open}`"), q, BTreeMap::new());
    }
    let q = deepest(4 * MAX_CLAUSES, |n| {
        vec!["MATCH (a:N) RETURN a.i AS i"; n].join(" UNION ")
    })
    .unwrap();
    push("union chain".into(), q, BTreeMap::new());

    // Long patterns are not nested, but a walk over them may recurse per
    // hop.
    let hops = HOPS;
    let walk = "-[:NEXT]->()".repeat(hops);
    for (name, query) in [
        (
            "long MATCH pattern",
            format!("MATCH (a:C {{i: 0}}){walk} RETURN count(*) AS c"),
        ),
        (
            "long OPTIONAL MATCH pattern",
            format!("MATCH (a:C {{i: 0}}) OPTIONAL MATCH (a){walk} RETURN count(*) AS c"),
        ),
        (
            "long path",
            format!("MATCH p = (a:C {{i: 0}}){walk} RETURN length(p) AS c"),
        ),
        (
            "long CREATE pattern",
            format!("CREATE (a:L){} RETURN 1 AS c", "-[:R]->(:L)".repeat(hops)),
        ),
        (
            "long MERGE pattern",
            format!("MERGE (a:M){} RETURN 1 AS c", "-[:R]->(:M)".repeat(hops)),
        ),
        (
            "long EXISTS pattern",
            format!("MATCH (a:C {{i: 0}}) WHERE EXISTS {{ (a){walk} }} RETURN 1 AS c"),
        ),
        (
            "long COUNT pattern",
            format!("MATCH (a:C {{i: 0}}) RETURN COUNT {{ (a){walk} }} AS c"),
        ),
        (
            "long pattern comprehension",
            format!("MATCH (a:C {{i: 0}}) RETURN [(a){walk} | 1] AS c"),
        ),
        (
            "long pattern predicate",
            format!("MATCH (a:C {{i: 0}}) WHERE (a){walk} RETURN 1 AS c"),
        ),
        (
            "many patterns",
            format!(
                "MATCH (a0:N {{i: 1}}){} RETURN count(*) AS c",
                (1..hops)
                    .map(|i| format!(", (a{i}:N {{i: 1}})"))
                    .collect::<String>()
            ),
        ),
        (
            "many projections",
            format!(
                "RETURN 1 AS c0{}",
                (1..hops)
                    .map(|i| format!(", {i} AS c{i}"))
                    .collect::<String>()
            ),
        ),
        (
            "long list",
            format!("RETURN [{}] AS c", vec!["1"; 50_000].join(", ")),
        ),
        (
            "long string",
            format!("RETURN '{}' AS c", "x".repeat(1_000_000)),
        ),
        (
            "many labels",
            format!(
                "MATCH (a{}) RETURN a",
                (0..hops).map(|i| format!(":L{i}")).collect::<String>()
            ),
        ),
        (
            "many SET items",
            format!(
                "MATCH (a:N) SET {} RETURN a",
                (0..hops)
                    .map(|i| format!("a.p{i} = {i}"))
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
        ),
    ] {
        push(name.to_string(), query, BTreeMap::new());
    }

    // Values at the nesting limit.
    let deep_list = nested_list(MAX_VALUE_NESTING);
    let deep_map = nested_map(MAX_VALUE_NESTING);
    for (name, value) in [("list", &deep_list), ("map", &deep_map)] {
        for query in [
            "RETURN $p AS x",
            "UNWIND [1, 2] AS i WITH $p AS x RETURN DISTINCT x",
            "UNWIND [1, 2] AS i WITH $p AS x ORDER BY x RETURN x",
            "RETURN $p = $p AS same, $p < $p AS less",
            "RETURN json.encode($p) AS s",
            "RETURN json.decode(json.encode($p)) AS v",
            "RETURN toString($p) AS s",
            "WITH $p AS x RETURN x, count(*) AS c",
            "CREATE (n:Deep {p: $p}) RETURN n.p AS p",
            "MATCH (n:Deep) RETURN n",
            "MATCH (n:Deep) DELETE n",
        ] {
            let mut params = BTreeMap::new();
            params.insert("p".to_string(), value.clone());
            push(format!("{name} value: {query}"), query.to_string(), params);
        }
    }
    let q = format!(
        "RETURN reduce(acc = 1, i IN range(1, {}) | [acc]) AS x",
        MAX_VALUE_NESTING
    );
    push("built list".into(), q, BTreeMap::new());
    let q = format!(
        "RETURN reduce(acc = 1, i IN range(1, {}) | {{a: acc}}) AS x",
        MAX_VALUE_NESTING
    );
    push("built map".into(), q, BTreeMap::new());

    // Past the limits: these must fail cleanly, not overflow on the way.
    push(
        "brackets past the limit".into(),
        format!("RETURN {} AS x", nest("[", "1", "]", 100_000)),
        BTreeMap::new(),
    );
    push(
        "chain past the limit".into(),
        format!("RETURN {} AS x", vec!["1"; 100_000].join(" + ")),
        BTreeMap::new(),
    );
    push(
        "reduce past the limit".into(),
        "RETURN size(reduce(acc = [], i IN range(1, 10000) | [acc])) AS x".into(),
        BTreeMap::new(),
    );
    out.push(DeepQuery {
        name: "parameter past the limit".into(),
        query: "RETURN $p AS x".into(),
        params: BTreeMap::new(),
        too_deep: Some(1_000_000),
    });
    out
}
