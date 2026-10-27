//! G-63: `$x IN n.list` seeks the list elements of a RANGE index.
//! G-72: so does an OR of such tests on one list, or `any(x IN $list WHERE
//! x IN n.list)`, one seek per element.
//!
//! A RANGE index on a property that holds lists keeps each list whole (for
//! `n.list = [..]`) and by its distinct elements (for `$x IN n.list` and
//! `any(x IN n.list WHERE x = $x)`). Each result check compares the seek
//! against `WITH * WHERE ...`, which keeps the condition above the scan and
//! so acts as the reference, over lists with duplicates, nulls, mixed
//! numbers and nested values, across writes, a WAL replay and a snapshot.

mod test_helpers;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use lora_database::{
    Database, ExecuteOptions, InMemoryGraph, LoraValue, PlanTreeNode, ResultFormat,
};
use lora_store::GraphStorage;
use lora_wal::{SyncMode, WalConfig};
use test_helpers::TestDb;

// ---------- helpers ----------

fn params(pairs: &[(&str, LoraValue)]) -> BTreeMap<String, LoraValue> {
    pairs
        .iter()
        .map(|(k, v)| (k.to_string(), v.clone()))
        .collect()
}

fn s(v: &str) -> LoraValue {
    LoraValue::String(v.to_string())
}

fn plan_of(
    db: &Database<InMemoryGraph>,
    query: &str,
    p: &BTreeMap<String, LoraValue>,
) -> PlanTreeNode {
    db.explain(query, Some(p.clone())).unwrap().tree.root
}

fn find<'a>(node: &'a PlanTreeNode, op: &str, out: &mut Vec<&'a PlanTreeNode>) {
    if node.operator == op {
        out.push(node);
    }
    for child in &node.children {
        find(child, op, out);
    }
}

fn ops<'a>(node: &'a PlanTreeNode, op: &str) -> Vec<&'a PlanTreeNode> {
    let mut out = Vec::new();
    find(node, op, &mut out);
    out
}

/// The element seeks of a plan, as `(key, value)`.
fn element_seeks(root: &PlanTreeNode) -> Vec<(String, String)> {
    ops(root, "NodeByPropertyScan")
        .into_iter()
        .filter(|n| n.details.get("mode").map(String::as_str) == Some("element"))
        .map(|n| (n.details["key"].clone(), n.details["value"].clone()))
        .collect()
}

/// The any-element seeks of a plan (G-72), as `(key, value)`.
fn any_seeks(root: &PlanTreeNode) -> Vec<(String, String)> {
    ops(root, "NodeByPropertyScan")
        .into_iter()
        .filter(|n| n.details.get("mode").map(String::as_str) == Some("any-element"))
        .map(|n| (n.details["key"].clone(), n.details["value"].clone()))
        .collect()
}

fn sorted_rows(
    db: &Database<InMemoryGraph>,
    query: &str,
    p: &BTreeMap<String, LoraValue>,
) -> Vec<String> {
    let result = db
        .execute_with_params(
            query,
            Some(ExecuteOptions {
                format: ResultFormat::Rows,
            }),
            p.clone(),
        )
        .unwrap_or_else(|e| panic!("{query}: {e}"));
    let json = serde_json::to_value(&result).unwrap();
    let mut rows: Vec<String> = json["rows"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|r| r.to_string())
        .collect();
    rows.sort();
    rows
}

/// The rows of `query` (which seeks) equal those of its `WITH * WHERE`
/// form (which filters a label scan); returns them.
fn assert_same_as_scan(
    db: &Database<InMemoryGraph>,
    query: &str,
    p: &BTreeMap<String, LoraValue>,
) -> Vec<String> {
    let (head, tail) = query.split_once(" WHERE ").expect("query has a WHERE");
    let (cond, rest) = tail.split_once(" RETURN ").expect("query has a RETURN");
    let reference = format!("{head} WITH * WHERE {cond} RETURN {rest}");
    let reference_plan = plan_of(db, &reference, p);
    assert!(
        element_seeks(&reference_plan).is_empty()
            && any_seeks(&reference_plan).is_empty()
            && ops(&reference_plan, "RelByListElementScan").is_empty(),
        "the reference must not seek"
    );
    let got = sorted_rows(db, query, p);
    assert_eq!(
        got,
        sorted_rows(db, &reference, p),
        "\nquery:     {query}\nreference: {reference}\nparams:    {p:?}"
    );
    got
}

fn run(db: &Database<InMemoryGraph>, query: &str) {
    db.execute(
        query,
        Some(ExecuteOptions {
            format: ResultFormat::Rows,
        }),
    )
    .unwrap_or_else(|e| panic!("{query}: {e}"));
}

fn run_p(db: &Database<InMemoryGraph>, query: &str, p: BTreeMap<String, LoraValue>) {
    db.execute_with_params(
        query,
        Some(ExecuteOptions {
            format: ResultFormat::Rows,
        }),
        p,
    )
    .unwrap_or_else(|e| panic!("{query}: {e}"));
}

/// xorshift64*: a fixed-seed generator, so a failure reproduces.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }
}

/// Values lists are drawn from and needles are sought by: numbers that
/// compare equal across integer and float, a negative zero, integers past
/// 2^53 that share an `f64` image, strings, booleans and nested values.
fn pool() -> Vec<LoraValue> {
    let big = 1i64 << 53;
    vec![
        s("techno"),
        s("house"),
        s("rock"),
        s(""),
        LoraValue::Int(1),
        LoraValue::Float(1.0),
        LoraValue::Int(2),
        LoraValue::Float(2.5),
        LoraValue::Float(-0.0),
        LoraValue::Int(0),
        LoraValue::Int(big),
        LoraValue::Int(big + 1),
        LoraValue::Float(big as f64),
        LoraValue::Bool(true),
        LoraValue::Bool(false),
        LoraValue::List(vec![s("techno")]),
        LoraValue::List(vec![LoraValue::Int(1), LoraValue::Int(2)]),
        LoraValue::List(vec![LoraValue::Float(1.0), LoraValue::Int(2)]),
        LoraValue::Map(BTreeMap::from([("k".to_string(), s("techno"))])),
    ]
}

/// A random list over the pool: duplicates and nulls included.
fn random_list(rng: &mut Rng, pool: &[LoraValue]) -> LoraValue {
    let len = rng.below(5);
    LoraValue::List(
        (0..len)
            .map(|_| {
                if rng.below(8) == 0 {
                    LoraValue::Null
                } else {
                    pool[rng.below(pool.len() as u64) as usize].clone()
                }
            })
            .collect(),
    )
}

/// `n` `:E` nodes with a random list (or a scalar, or nothing) in `l`.
fn seed_random(db: &Database<InMemoryGraph>, rng: &mut Rng, n: usize) {
    let pool = pool();
    for i in 0..n {
        let l = match rng.below(10) {
            0 => {
                run_p(
                    db,
                    "CREATE (:E {id: $id})",
                    params(&[("id", LoraValue::Int(i as i64))]),
                );
                continue;
            }
            // A scalar is not a list: `x IN 'techno'` is false.
            1 => pool[rng.below(pool.len() as u64) as usize].clone(),
            _ => random_list(rng, &pool),
        };
        run_p(
            db,
            "CREATE (:E {id: $id, l: $l})",
            params(&[("id", LoraValue::Int(i as i64)), ("l", l)]),
        );
    }
}

/// Needles: every pool value, null, and values no list holds.
fn needles() -> Vec<LoraValue> {
    let mut out = pool();
    out.push(LoraValue::Null);
    out.push(s("missing"));
    out.push(LoraValue::Int(99));
    out.push(LoraValue::Float(f64::NAN));
    out.push(LoraValue::List(vec![]));
    out
}

const SEEK: &str = "MATCH (n:E) WHERE $g IN n.l RETURN n.id AS id";
const ANY: &str = "MATCH (n:E) WHERE any(x IN n.l WHERE x = $g) RETURN n.id AS id";

/// Every needle finds by seek what the scan finds, under both forms.
fn assert_parity(db: &Database<InMemoryGraph>) {
    for needle in needles() {
        let p = params(&[("g", needle.clone())]);
        for q in [SEEK, ANY] {
            assert_eq!(element_seeks(&plan_of(db, q, &p)).len(), 1, "{q} must seek");
            let rows = assert_same_as_scan(db, q, &p);
            let mut ids = rows.clone();
            ids.dedup();
            assert_eq!(
                ids.len(),
                rows.len(),
                "a node came back twice for {needle:?}"
            );
        }
    }
    assert_index_matches_rebuild(db);
}

/// The index holds exactly what one built from the current data holds:
/// the candidates of every needle and the element counts. Stale entries
/// would not show in results (the filter above the seek drops them).
fn assert_index_matches_rebuild(db: &Database<InMemoryGraph>) {
    let probe = |db: &Database<InMemoryGraph>| {
        db.with_store(|g| {
            let candidates: Vec<Option<Vec<u64>>> = needles()
                .into_iter()
                .filter_map(|g| lora_executor::lora_value_to_property(g).ok())
                .map(|v| g.node_list_element_candidates("E", "l", &v))
                .collect();
            (candidates, g.graph_stats().node_list_elements)
        })
    };
    let maintained = probe(db);
    run(db, "DROP INDEX e_l");
    run(db, "CREATE INDEX e_l FOR (n:E) ON (n.l)");
    assert_eq!(maintained, probe(db), "maintained vs rebuilt element index");
}

fn indexed_db() -> Database<InMemoryGraph> {
    let db = Database::in_memory();
    run(&db, "CREATE INDEX e_l FOR (n:E) ON (n.l)");
    db
}

// ---------- planning ----------

/// The Festimap repro: 5,000 events, a RANGE index on the list property.
fn repro() -> TestDb {
    let db = TestDb::new();
    db.run("CREATE INDEX ev_gl FOR (n:Event) ON (n.genreLineage)");
    db.run(
        "UNWIND range(1, 5000) AS i CREATE (:Event {key: i, city: 'c' + toString(i % 100), \
         genreLineage: CASE WHEN i % 50 = 0 THEN ['electronic','techno'] ELSE ['rock-metal'] END})",
    );
    db
}

#[test]
fn element_test_plans_an_index_seek() {
    let db = repro();
    let p = params(&[("g", s("techno"))]);
    for (q, value) in [
        (
            "MATCH (e:Event) WHERE $g IN e.genreLineage RETURN e",
            "Parameter(\"g\")",
        ),
        (
            "MATCH (e:Event) WHERE 'techno' IN e.genreLineage RETURN e",
            "Literal(String(\"techno\"))",
        ),
        (
            "MATCH (e:Event) WHERE any(x IN e.genreLineage WHERE x = $g) RETURN e",
            "Parameter(\"g\")",
        ),
        (
            "MATCH (e:Event) WHERE any(x IN e.genreLineage WHERE $g = x) RETURN e",
            "Parameter(\"g\")",
        ),
        (
            "MATCH (e:Event) WHERE e.key > 0 AND $g IN e.genreLineage RETURN e",
            "Parameter(\"g\")",
        ),
    ] {
        let root = plan_of(&db.service, q, &p);
        let seeks = element_seeks(&root);
        assert_eq!(seeks.len(), 1, "{q}: {root:#?}");
        assert_eq!(seeks[0].0, "genreLineage");
        assert!(seeks[0].1.contains(value), "{q}: {}", seeks[0].1);
        assert!(ops(&root, "NodeByLabelScan").is_empty(), "{q}: {root:#?}");
        // Average entities per element: 5,100 postings over 3 elements.
        let seek = ops(&root, "NodeByPropertyScan")[0];
        assert_eq!(seek.estimated_rows, Some(1700), "{q}");
    }
    assert_eq!(
        db.run_with_params(
            "MATCH (e:Event) WHERE $g IN e.genreLineage RETURN e.key AS k",
            p.clone()
        )
        .len(),
        100
    );
}

#[test]
fn shapes_that_cannot_seek_keep_the_scan() {
    let db = repro();
    let p = params(&[("g", s("techno"))]);
    for q in [
        // No index on the property.
        "MATCH (e:Event) WHERE $g IN e.city RETURN e",
        // The list does not come from the scanned node.
        "MATCH (e:Event) WITH e, ['techno'] AS l WHERE e.city IN l RETURN e",
        // The value depends on the node itself.
        "MATCH (e:Event) WHERE e.city IN e.genreLineage RETURN e",
        // any() with another test than equality to the element.
        "MATCH (e:Event) WHERE any(x IN e.genreLineage WHERE x STARTS WITH $g) RETURN e",
        "MATCH (e:Event) WHERE any(x IN e.genreLineage WHERE x = e.city) RETURN e",
        // all() / none() are not membership.
        "MATCH (e:Event) WHERE all(x IN e.genreLineage WHERE x = $g) RETURN e",
        // A disjunction with another test is not an element seek.
        "MATCH (e:Event) WHERE $g IN e.genreLineage OR e.key = 1 RETURN e",
        // An OR over two list properties, or two nodes.
        "MATCH (e:Event) WHERE $g IN e.genreLineage OR $g IN e.tags RETURN e",
        "MATCH (e:Event), (f:Event) WHERE $g IN e.genreLineage OR $g IN f.genreLineage RETURN e",
        // A disjunct that is itself a conjunction.
        "MATCH (e:Event) WHERE $g IN e.genreLineage OR ($g IN e.genreLineage AND e.key = 1) RETURN e",
        // any() whose list depends on the node, or whose test is not membership.
        "MATCH (e:Event) WHERE any(x IN [e.city] WHERE x IN e.genreLineage) RETURN e",
        "MATCH (e:Event) WHERE any(x IN $gs WHERE x STARTS WITH e.city) RETURN e",
    ] {
        let root = plan_of(&db.service, q, &p);
        assert!(
            element_seeks(&root).is_empty() && any_seeks(&root).is_empty(),
            "{q} must not seek: {root:#?}"
        );
    }
    // Without a RANGE index the element test scans, as before.
    let bare = TestDb::new();
    bare.run("CREATE (:Event {genreLineage: ['techno']})");
    let q = "MATCH (e:Event) WHERE $g IN e.genreLineage RETURN e";
    assert!(element_seeks(&plan_of(&bare.service, q, &p)).is_empty());
}

/// With an indexed equality beside it, the planner seeks whichever finds
/// fewer rows: a city (50 events each) over a common genre, a rare genre
/// over a common city.
#[test]
fn the_more_selective_seek_wins() {
    let db = TestDb::new();
    db.run("CREATE INDEX ev_gl FOR (n:Event) ON (n.genreLineage)");
    db.run("CREATE INDEX ev_city FOR (n:Event) ON (n.city)");
    // 100 cities of 50 events; 100 genres of ~100 events; one genre on
    // every event.
    db.run(
        "UNWIND range(1, 5000) AS i CREATE (:Event {key: i, city: 'c' + toString(i % 100), \
         genreLineage: ['music', 'g' + toString(i % 100), 'g' + toString((i + 1) % 100)]})",
    );
    let q = "MATCH (e:Event) WHERE e.city = $c AND $g IN e.genreLineage RETURN e.key AS k";
    let p = params(&[("c", s("c7")), ("g", s("g7"))]);
    let root = plan_of(&db.service, q, &p);
    // Per element: 15,000 postings / 101 elements = 149 > 50 per city.
    let seek = ops(&root, "NodeByPropertyScan");
    assert_eq!(seek.len(), 1, "{root:#?}");
    assert_eq!(seek[0].details["key"], "city", "{root:#?}");
    assert_eq!(assert_same_as_scan(&db.service, q, &p).len(), 50);

    // A city every event shares: the genre seek wins.
    let db = TestDb::new();
    db.run("CREATE INDEX ev_gl FOR (n:Event) ON (n.genreLineage)");
    db.run("CREATE INDEX ev_city FOR (n:Event) ON (n.city)");
    db.run(
        "UNWIND range(1, 5000) AS i CREATE (:Event {key: i, city: 'Berlin', \
         genreLineage: CASE WHEN i % 50 = 0 THEN ['techno'] ELSE ['g' + toString(i % 49)] END})",
    );
    let p = params(&[("c", s("Berlin")), ("g", s("techno"))]);
    let root = plan_of(&db.service, q, &p);
    assert_eq!(element_seeks(&root).len(), 1, "{root:#?}");
    assert_eq!(assert_same_as_scan(&db.service, q, &p).len(), 100);
}

/// A keyset page over an element filter: the ordered walk of the `key`
/// index or the element seek, whichever the planner picks, pages right.
#[test]
fn keyset_pages_over_an_element_filter() {
    let db = repro();
    db.run("CREATE INDEX ev_key FOR (n:Event) ON (n.key)");
    let q = "MATCH (e:Event) WHERE $g IN e.genreLineage AND e.key > $after \
             RETURN e.key AS k ORDER BY e.key LIMIT 7";
    let mut after = 0i64;
    let mut seen = Vec::new();
    loop {
        let p = params(&[("g", s("techno")), ("after", LoraValue::Int(after))]);
        let rows = db.run_with_params(q, p);
        if rows.is_empty() {
            break;
        }
        for row in &rows {
            let k = row["k"].as_i64().unwrap();
            assert!(k > after);
            seen.push(k);
        }
        after = *seen.last().unwrap();
    }
    assert_eq!(seen, (1..=100).map(|i| i * 50).collect::<Vec<_>>());
}

/// `n.list = [..]` still seeks the whole value, beside the elements.
#[test]
fn whole_list_equality_still_seeks() {
    let db = repro();
    let q = "MATCH (e:Event) WHERE e.genreLineage = ['electronic', 'techno'] RETURN e.key AS k";
    let root = plan_of(&db.service, q, &BTreeMap::new());
    let seeks = ops(&root, "NodeByPropertyScan");
    assert_eq!(seeks.len(), 1, "{root:#?}");
    assert!(!seeks[0].details.contains_key("mode"));
    assert_eq!(db.run(q).len(), 100);
    let q = "MATCH (e:Event) WHERE e.genreLineage > ['electronic'] AND \
             e.genreLineage < ['electronic', 'zzz'] RETURN e.key AS k";
    // A range over lists seeks the whole values, and keeps the rows the
    // comparison holds for (it once dropped every list).
    let root = plan_of(&db.service, q, &BTreeMap::new());
    assert_eq!(ops(&root, "NodeByPropertyRangeScan").len(), 1, "{root:#?}");
    assert_eq!(
        assert_same_as_scan(&db.service, q, &BTreeMap::new()).len(),
        100
    );
    let q = "MATCH (e:Event) WHERE e.genreLineage >= ['rock-metal'] RETURN e.key AS k";
    assert_eq!(
        assert_same_as_scan(&db.service, q, &BTreeMap::new()).len(),
        4900
    );
    // Booleans, too, were dropped by the range refilter.
    db.run("CREATE INDEX ev_flag FOR (n:Event) ON (n.flag)");
    db.run("MATCH (e:Event) SET e.flag = e.key % 3 = 0");
    let q = "MATCH (e:Event) WHERE e.flag > false RETURN e.key AS k";
    let root = plan_of(&db.service, q, &BTreeMap::new());
    assert_eq!(ops(&root, "NodeByPropertyRangeScan").len(), 1, "{root:#?}");
    assert_eq!(
        assert_same_as_scan(&db.service, q, &BTreeMap::new()).len(),
        1666
    );
}

// ---------- results ----------

#[test]
fn seek_matches_the_scan_over_random_lists() {
    let db = indexed_db();
    let mut rng = Rng(0x9E37_79B9_7F4A_7C15);
    seed_random(&db, &mut rng, 400);
    // Values the index has no key for: never sought, still in lists.
    run(
        &db,
        "CREATE (:E {id: 1000, l: [duration('P1D'), 'techno']})",
    );
    run(&db, "CREATE (:E {id: 1001, l: [point({x: 1, y: 2}), 1]})");
    assert_parity(&db);

    // Spot checks of the semantics the reference implements.
    let ids = |g: LoraValue| sorted_rows(&db, SEEK, &params(&[("g", g)]));
    let fresh = indexed_db();
    run(&fresh, "CREATE (:E {id: 1, l: [1, 1, null, 'a']}), (:E {id: 2, l: [1.0]}), (:E {id: 3, l: [null]}), (:E {id: 4, l: []})");
    let fresh_ids = |g: LoraValue| sorted_rows(&fresh, SEEK, &params(&[("g", g)]));
    assert_eq!(
        fresh_ids(LoraValue::Int(1)),
        vec![r#"{"id":1}"#, r#"{"id":2}"#]
    );
    assert_eq!(
        fresh_ids(LoraValue::Float(1.0)),
        vec![r#"{"id":1}"#, r#"{"id":2}"#]
    );
    assert!(fresh_ids(LoraValue::Null).is_empty());
    assert!(
        fresh_ids(s("b")).is_empty(),
        "a null element and no match is null"
    );
    assert_eq!(fresh_ids(s("a")), vec![r#"{"id":1}"#]);
    assert!(!ids(s("techno")).is_empty());
}

#[test]
fn seek_stays_correct_across_writes() {
    let db = indexed_db();
    let mut rng = Rng(0xD1B5_4A32_D192_ED03);
    seed_random(&db, &mut rng, 150);
    assert_parity(&db);

    let pool = pool();
    for round in 0..6 {
        for id in 0..150i64 {
            let p = |extra: &[(&str, LoraValue)]| {
                let mut p = params(&[("id", LoraValue::Int(id))]);
                p.extend(extra.iter().map(|(k, v)| (k.to_string(), v.clone())));
                p
            };
            match (rng.below(9), round) {
                // Replace the list.
                (0, _) => run_p(
                    &db,
                    "MATCH (n:E {id: $id}) SET n.l = $l",
                    p(&[("l", random_list(&mut rng, &pool))]),
                ),
                // Append to it.
                (1, _) => run_p(
                    &db,
                    "MATCH (n:E {id: $id}) WHERE n.l IS NOT NULL AND type.of(n.l) STARTS WITH 'LIST' \
                     SET n.l = n.l + $x",
                    p(&[("x", pool[rng.below(pool.len() as u64) as usize].clone())]),
                ),
                (2, _) => run_p(&db, "MATCH (n:E {id: $id}) SET n.l = null", p(&[])),
                (3, _) => run_p(&db, "MATCH (n:E {id: $id}) REMOVE n.l", p(&[])),
                // Out of the label, and back.
                (4, _) => run_p(&db, "MATCH (n:E {id: $id}) REMOVE n:E SET n:Gone", p(&[])),
                (5, _) => run_p(&db, "MATCH (n:Gone {id: $id}) REMOVE n:Gone SET n:E", p(&[])),
                // Replace every property.
                (6, _) => run_p(
                    &db,
                    "MATCH (n:E {id: $id}) SET n = {id: $id, l: $l}",
                    p(&[("l", random_list(&mut rng, &pool))]),
                ),
                (7, r) if r >= 4 => run_p(&db, "MATCH (n:E {id: $id}) DETACH DELETE n", p(&[])),
                _ => {}
            }
        }
        assert_parity(&db);
    }

    // A rolled-back write leaves the index as it was.
    let before = sorted_rows(&db, SEEK, &params(&[("g", s("techno"))]));
    let err = db.execute(
        "MATCH (n:E) SET n.l = ['techno'] WITH count(*) AS c RETURN 1 / 0",
        None,
    );
    assert!(err.is_err());
    assert_eq!(
        sorted_rows(&db, SEEK, &params(&[("g", s("techno"))])),
        before
    );
    assert_parity(&db);
}

// ---------- restart ----------

struct TmpDir {
    path: PathBuf,
}

impl TmpDir {
    fn new(tag: &str) -> Self {
        let mut path = std::env::temp_dir();
        path.push(format!(
            "lora-list-element-{}-{}-{}",
            tag,
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&path).unwrap();
        Self { path }
    }

    fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for TmpDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

fn wal(dir: &Path) -> WalConfig {
    WalConfig::Enabled {
        dir: dir.to_path_buf(),
        sync_mode: SyncMode::GroupSync { interval_ms: 50 },
        segment_target_bytes: 8 * 1024 * 1024,
        truncate_corrupt_tail: false,
    }
}

/// Every needle's rows, plus the planner stats.
fn observe(db: &Database<InMemoryGraph>) -> (Vec<Vec<String>>, lora_store::GraphStats) {
    let rows = needles()
        .into_iter()
        .map(|g| sorted_rows(db, SEEK, &params(&[("g", g)])))
        .collect();
    (rows, db.with_store(|g| g.graph_stats()))
}

fn write_history(db: &Database<InMemoryGraph>) {
    let mut rng = Rng(0xA076_1D64_78BD_642F);
    // Data before the index: the CREATE INDEX backfills it.
    seed_random(db, &mut rng, 60);
    run(db, "CREATE INDEX e_l FOR (n:E) ON (n.l)");
    // Data after it, then writes to the indexed lists.
    for i in 60..120 {
        run_p(
            db,
            "CREATE (:E {id: $id, l: $l})",
            params(&[
                ("id", LoraValue::Int(i)),
                ("l", random_list(&mut rng, &pool())),
            ]),
        );
    }
    run(
        db,
        "MATCH (n:E) WHERE n.id % 7 = 0 SET n.l = ['techno', 'techno', null]",
    );
    run(db, "MATCH (n:E) WHERE n.id % 11 = 0 AND type.of(n.l) STARTS WITH 'LIST' SET n.l = n.l + 'house'");
    run(db, "MATCH (n:E) WHERE n.id % 13 = 0 SET n.l = null");
    run(db, "MATCH (n:E) WHERE n.id % 17 = 0 REMOVE n.l");
    run(db, "MATCH (n:E) WHERE n.id % 19 = 0 DETACH DELETE n");
}

#[test]
fn wal_replay_restores_the_element_index() {
    let dir = TmpDir::new("wal");
    let written;
    {
        let db = Database::open_with_wal(wal(dir.path())).unwrap();
        write_history(&db);
        assert_parity(&db);
        written = observe(&db);
        assert!(!written.1.node_list_elements.is_empty());
    }
    let reopened = Database::open_with_wal(wal(dir.path())).unwrap();
    assert_eq!(observe(&reopened), written);
    assert_parity(&reopened);
}

#[test]
fn snapshot_load_restores_the_element_index() {
    let dir = TmpDir::new("snapshot");
    let path = dir.path().join("snap.bin");
    let db = Database::in_memory();
    write_history(&db);
    let written = observe(&db);
    db.save_snapshot_to(&path).unwrap();
    let loaded = Database::in_memory_from_snapshot(&path).unwrap();
    assert_eq!(observe(&loaded), written);
    assert_parity(&loaded);
    assert_eq!(
        db.with_store(|g| g.plan_signature()),
        loaded.with_store(|g| g.plan_signature())
    );
}

// ---------- relationships ----------

#[test]
fn relationship_element_test_seeks_its_type() {
    let db = Database::in_memory();
    run(&db, "CREATE INDEX t_tags FOR ()-[r:T]-() ON (r.tags)");
    let mut rng = Rng(0x5851_F42D_4C95_7F2D);
    let pool = pool();
    for i in 0..120i64 {
        run_p(
            &db,
            "CREATE (:A {id: $i})-[:T {id: $i, tags: $l}]->(:B {id: $i})",
            params(&[
                ("i", LoraValue::Int(i)),
                ("l", random_list(&mut rng, &pool)),
            ]),
        );
    }
    // Writes after the fact: replace, append, clear, delete.
    run(
        &db,
        "MATCH ()-[r:T]->() WHERE r.id % 5 = 0 SET r.tags = ['techno', 'techno']",
    );
    run(&db, "MATCH ()-[r:T]->() WHERE r.id % 7 = 0 AND type.of(r.tags) STARTS WITH 'LIST' SET r.tags = r.tags + 1.0");
    run(
        &db,
        "MATCH ()-[r:T]->() WHERE r.id % 11 = 0 SET r.tags = null",
    );
    run(&db, "MATCH ()-[r:T]->() WHERE r.id % 13 = 0 DELETE r");

    for (q, seeks) in [
        (
            "MATCH (a)-[r:T]->(b) WHERE $g IN r.tags RETURN a.id AS a, r.id AS r, b.id AS b",
            true,
        ),
        (
            "MATCH (a)-[r:T]-(b) WHERE $g IN r.tags RETURN a.id AS a, r.id AS r, b.id AS b",
            true,
        ),
        (
            "MATCH (a)<-[r:T]-(b) WHERE any(x IN r.tags WHERE x = $g) RETURN a.id AS a, r.id AS r",
            true,
        ),
        // No index on the property.
        (
            "MATCH (a)-[r:T]->(b) WHERE $g IN r.other RETURN r.id AS r",
            false,
        ),
    ] {
        for needle in needles() {
            let p = params(&[("g", needle)]);
            let root = plan_of(&db, q, &p);
            assert_eq!(
                ops(&root, "RelByListElementScan").len(),
                usize::from(seeks),
                "{q}: {root:#?}"
            );
            if seeks {
                assert_same_as_scan(&db, q, &p);
            }
        }
    }
    let p = params(&[("g", s("techno"))]);
    let rows = sorted_rows(
        &db,
        "MATCH ()-[r:T]->() WHERE $g IN r.tags RETURN r.id AS r",
        &p,
    );
    assert!(rows.len() >= 20, "{rows:?}");
}

// ---------- G-72: an OR of element tests ----------

#[test]
fn an_or_of_element_tests_seeks_each_element() {
    let db = repro();
    let p = params(&[
        ("g", s("techno")),
        ("h", s("house")),
        ("gs", LoraValue::List(vec![s("techno"), s("house")])),
    ]);
    // Average entities per element: 5,100 postings over 3 elements.
    for (q, value) in [
        (
            "MATCH (e:Event) WHERE $g IN e.genreLineage OR $h IN e.genreLineage RETURN e.key AS k",
            "List([Parameter(\"g\"), Parameter(\"h\")])",
        ),
        (
            "MATCH (e:Event) WHERE $g IN e.genreLineage \
             OR any(x IN e.genreLineage WHERE x = 'acid') RETURN e.key AS k",
            "List([Parameter(\"g\"), Literal(String(\"acid\"))])",
        ),
        (
            "MATCH (e:Event) WHERE any(x IN $gs WHERE x IN e.genreLineage) RETURN e.key AS k",
            "Parameter(\"gs\")",
        ),
        (
            "MATCH (e:Event) WHERE any(x IN e.genreLineage WHERE x IN $gs) RETURN e.key AS k",
            "Parameter(\"gs\")",
        ),
        (
            "MATCH (e:Event) WHERE e.key > 0 AND ($g IN e.genreLineage OR $h IN e.genreLineage) \
             RETURN e.key AS k",
            "List([Parameter(\"g\"), Parameter(\"h\")])",
        ),
    ] {
        let root = plan_of(&db.service, q, &p);
        let seeks = any_seeks(&root);
        assert_eq!(seeks.len(), 1, "{q}: {root:#?}");
        assert_eq!(seeks[0].0, "genreLineage");
        assert_eq!(seeks[0].1, value, "{q}");
        assert!(ops(&root, "NodeByLabelScan").is_empty(), "{q}: {root:#?}");
        let seek = ops(&root, "NodeByPropertyScan")[0];
        // EXPLAIN shows the per-element estimate; the planner's score
        // multiplies it by the literal list's length.
        assert_eq!(seek.estimated_rows, Some(1700), "{q}");
        // Every event holding techno, once although it holds two picks.
        assert_eq!(assert_same_as_scan(&db.service, q, &p).len(), 100, "{q}");
    }
    // Three picks average 5,100 rows here, more than the 5,000 events: the
    // label scan stays.
    let q = "MATCH (e:Event) WHERE $g IN e.genreLineage OR $h IN e.genreLineage \
             OR 'acid' IN e.genreLineage RETURN e.key AS k";
    let root = plan_of(&db.service, q, &p);
    assert!(any_seeks(&root).is_empty(), "{root:#?}");
    assert_eq!(assert_same_as_scan(&db.service, q, &p).len(), 100);
}

#[test]
fn an_or_of_element_tests_matches_the_scan_over_random_lists() {
    let db = indexed_db();
    let mut rng = Rng(0x2545_F491_4F6C_DD1D);
    seed_random(&db, &mut rng, 400);
    let needles = needles();
    let queries = [
        "MATCH (n:E) WHERE $g IN n.l OR $h IN n.l RETURN n.id AS id",
        "MATCH (n:E) WHERE any(x IN $gs WHERE x IN n.l) RETURN n.id AS id",
        "MATCH (n:E) WHERE any(x IN n.l WHERE x IN $gs) RETURN n.id AS id",
        "MATCH (n:E) WHERE $g IN n.l OR any(x IN n.l WHERE x = $h) OR 'rock' IN n.l \
         RETURN n.id AS id",
    ];
    for _ in 0..60 {
        let g = needles[rng.below(needles.len() as u64) as usize].clone();
        let h = needles[rng.below(needles.len() as u64) as usize].clone();
        // Duplicates and nulls in the list of picks, or none at all.
        let gs = match rng.below(6) {
            0 => LoraValue::List(vec![]),
            1 => LoraValue::Null,
            _ => LoraValue::List(vec![g.clone(), h.clone(), g.clone(), LoraValue::Null]),
        };
        let p = params(&[("g", g), ("h", h), ("gs", gs)]);
        for q in queries {
            assert_eq!(any_seeks(&plan_of(&db, q, &p)).len(), 1, "{q} must seek");
            let rows = assert_same_as_scan(&db, q, &p);
            let mut ids = rows.clone();
            ids.dedup();
            assert_eq!(ids.len(), rows.len(), "a node came back twice: {q} {p:?}");
        }
    }
}

#[test]
fn a_relationship_or_of_element_tests_seeks_its_type() {
    let db = Database::in_memory();
    run(&db, "CREATE INDEX t_tags FOR ()-[r:T]-() ON (r.tags)");
    let mut rng = Rng(0x9E37_79B9_7F4A_7C15);
    let pool = pool();
    for i in 0..150i64 {
        run_p(
            &db,
            "CREATE (:A {id: $i})-[:T {id: $i, tags: $l}]->(:B {id: $i})",
            params(&[
                ("i", LoraValue::Int(i)),
                ("l", random_list(&mut rng, &pool)),
            ]),
        );
    }
    let needles = needles();
    for q in [
        "MATCH (a)-[r:T]->(b) WHERE $g IN r.tags OR $h IN r.tags RETURN a.id AS a, r.id AS r",
        "MATCH (a)-[r:T]-(b) WHERE any(x IN $gs WHERE x IN r.tags) RETURN a.id AS a, r.id AS r",
        "MATCH (a)<-[r:T]-(b) WHERE any(x IN r.tags WHERE x IN $gs) RETURN r.id AS r",
    ] {
        for _ in 0..30 {
            let g = needles[rng.below(needles.len() as u64) as usize].clone();
            let h = needles[rng.below(needles.len() as u64) as usize].clone();
            let p = params(&[
                ("g", g.clone()),
                ("h", h.clone()),
                ("gs", LoraValue::List(vec![g, h, LoraValue::Null])),
            ]);
            let root = plan_of(&db, q, &p);
            let scans = ops(&root, "RelByListElementScan");
            assert_eq!(scans.len(), 1, "{q}: {root:#?}");
            assert_eq!(
                scans[0].details.get("mode").map(String::as_str),
                Some("any-element")
            );
            assert_same_as_scan(&db, q, &p);
        }
    }
}
