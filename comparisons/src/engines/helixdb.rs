//! HelixDB engine adapter.
//!
//! HelixDB is a vector-graph database written in Rust. The `helix-db`
//! crate on crates.io (imported as `helix_db`) is a **client SDK**, not
//! an embedded engine: it pairs a typed query-builder DSL with a small
//! async HTTP `Client` that POSTs `DynamicQueryRequest`s to a running
//! HelixDB instance at `/v1/query`. There is no in-process storage
//! backend exposed by the crate, so this adapter sits in the same
//! "server-only" bucket as `engines/memgraph.rs`: it expects a HelixDB
//! instance reachable at `http://127.0.0.1:6969` (override via
//! `HELIXDB_URL`). No Docker or process management is performed by the
//! bench itself — the user launches HelixDB out-of-band before running
//! `cargo bench --features helixdb`.
//!
//! "Fresh" in the [`fresh`] / [`fresh_persistent`] sense means *wiping
//! every node currently in the database* with a `drop()` write batch
//! and then handing back a client pointed at the same server, mirroring
//! how the embedded engines hand back a brand-new database object.
//!
//! ## Query strategy: sentinel strings
//!
//! Unlike the Cypher-speaking engines, HelixDB has no string query
//! language: queries are built programmatically as a typed AST. The
//! bench harness in `benches/comparison.rs` passes queries as strings,
//! so this adapter interprets the workload's string as a **sentinel
//! keyword** (`scan_label`, `lookup_by_id:500`, `one_hop`, …) and
//! dispatches to a hand-written DSL traversal in [`execute`]. The adapter
//! pushes work down to the DSL wherever it can: `where_`/`Predicate`/`Expr`
//! for predicates, `project` for computed returns, `aggregate_by` for
//! sum/min/max/avg, `order_by`/`range` for sort & paginate, edge scans
//! (`e_with_label_where`) for relationship-property filters, chained
//! `out`/`in_`/`both` for traversals, and `set_property`/`drop`/`add_n`/
//! `add_e` for writes. Only operations the DSL genuinely lacks carry a
//! `notes.helixdb` entry in `workloads.yml` instead: scalar string
//! functions (upper/substring/…), abs/floor/ceil/round, value-level
//! DISTINCT, `count()` (rate-limited on the enterprise-dev image), MERGE,
//! and range-bounded variable-length paths.

use std::sync::Arc;

use helix_db::dsl::prelude::*;
use helix_db::Client;
use serde::de::DeserializeOwned;
use serde::Deserialize;
use tokio::runtime::{Builder, Runtime};

use crate::engine::DbHandle;
use crate::fixtures::{Fixture, FixtureKind};

const DEFAULT_URL: &str = "http://127.0.0.1:6969";
/// How many `add_n` / `add_e` operations to bundle into a single write
/// batch. The HTTP round-trip dominates per-batch cost, so larger is
/// better; the cap exists to keep the JSON payload from getting absurd
/// at very large fixture sizes (1k nodes ~ a few hundred kB at 256).
const CHUNK: usize = 256;

/// HelixDB connection handle.
///
/// The tokio runtime is owned by the handle so the synchronous bench
/// harness can `block_on` async client calls without leaking a runtime
/// per iteration. Both fields are cheap to clone — `Client` is a thin
/// wrapper around a `reqwest::Client` — but we keep a single instance
/// per handle to match how other engines (kuzu, memgraph) share state.
pub struct HelixHandle {
    pub rt: Arc<Runtime>,
    pub client: Client,
}

fn build_runtime() -> Runtime {
    Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("helixdb: tokio runtime build failed")
}

fn helix_url() -> String {
    std::env::var("HELIXDB_URL").unwrap_or_else(|_| DEFAULT_URL.to_string())
}

fn open_client() -> Client {
    Client::new(Some(&helix_url())).expect("helixdb: client construction failed")
}

/// Deserialize target for queries we don't care about the body of —
/// HelixDB always returns a JSON object on success and `serde_json::Value`
/// would pull in `serde_json` just for this. `()` won't deserialize from
/// a JSON object, so we use a unit-like struct with `#[serde(default)]`.
#[derive(Deserialize, Default)]
struct Ignored {
    #[serde(default)]
    #[allow(dead_code)]
    _unused: (),
}

fn run_write(rt: &Runtime, client: &Client, req: DynamicQueryRequest) {
    rt.block_on(async { client.query::<Ignored>().dynamic_query(req).send().await })
        .unwrap_or_else(|e| panic!("helixdb write query failed: {e}"));
}

fn run_read<R: DeserializeOwned>(rt: &Runtime, client: &Client, req: DynamicQueryRequest) -> R {
    rt.block_on(async { client.query::<R>().dynamic_query(req).send().await })
        .unwrap_or_else(|e| panic!("helixdb read query failed: {e}"))
}

/// Wipe every node (and, transitively, every incident edge) currently
/// in the HelixDB instance. We don't know the label set up front, so
/// scan by `has_key("$label")` — that virtual property exists on every
/// node regardless of user label.
fn wipe(rt: &Runtime, client: &Client) {
    let req = DynamicQueryRequest::write(
        write_batch()
            .var_as(
                "_all",
                g().n_where(SourcePredicate::has_key("$label")).drop(),
            )
            .returning(["_all"]),
    );
    run_write(rt, client, req);
}

pub fn fresh() -> DbHandle {
    let rt = Arc::new(build_runtime());
    let client = open_client();
    wipe(&rt, &client);
    DbHandle::Helix(HelixHandle { rt, client }, None)
}

/// HelixDB has no separate "persistent" mode — the server is always
/// backed by its on-disk storage. The persistent column exists only so
/// the comparison matrix has a slot; functionally this is identical to
/// [`fresh`].
pub fn fresh_persistent() -> DbHandle {
    fresh()
}

pub fn seed(handle: &mut DbHandle, fixture: Fixture) {
    let DbHandle::Helix(h, _) = handle else {
        panic!("helixdb seed: wrong handle variant");
    };
    match fixture.kind {
        FixtureKind::Empty => {}
        FixtureKind::Nodes => seed_nodes(&h.rt, &h.client, fixture.size),
        FixtureKind::Chain => seed_chain(&h.rt, &h.client, fixture.size),
        FixtureKind::Star => seed_star(&h.rt, &h.client, fixture.size),
        FixtureKind::Social => seed_social(&h.rt, &h.client, fixture.size),
    }
}

/// HelixDB stores nodes in label-keyed structures and the DSL has no
/// explicit `CREATE INDEX` step for property lookups — equality and
/// range scans go through the same `n_where` predicate path. So this
/// is a no-op, mirroring the lora adapter.
pub fn create_index(_handle: &DbHandle, _prop: &str) {}

pub fn execute(handle: &DbHandle, q: &str) {
    let DbHandle::Helix(h, _) = handle else {
        panic!("helixdb execute: wrong handle variant");
    };
    dispatch(&h.rt, &h.client, q);
}

// ---------------------------------------------------------------------------
// Sentinel query dispatch
// ---------------------------------------------------------------------------

fn dispatch(rt: &Runtime, client: &Client, q: &str) {
    // Sentinel string format:
    //   "<keyword>"            — no parameter
    //   "<keyword>:<integer>"  — keyword with a single integer arg
    let (keyword, arg) = match q.split_once(':') {
        Some((k, v)) => (k, v.parse::<i64>().ok()),
        None => (q, None),
    };
    match keyword {
        // construct / bulk-seed workloads carry an empty sentinel: their
        // cost is the `fresh`/`seed` call, so there is no query to run.
        "" => {}
        // ---- scans & lookups ----
        "scan_label" => q_scan_label(rt, client),
        "scan_filtered" => q_scan_filtered(rt, client),
        "lookup_by_id" => q_lookup_by_id(rt, client, arg.expect("lookup_by_id: missing id arg")),
        // ---- predicates: server-side `where_` over the Node scan ----
        "where_or" => node_filter(
            rt,
            client,
            Predicate::or(vec![
                Predicate::eq("value", 10i64),
                Predicate::eq("value", 20i64),
            ]),
        ),
        "where_in_list" => node_filter(
            rt,
            client,
            Predicate::or(vec![
                Predicate::eq("value", 10i64),
                Predicate::eq("value", 20i64),
                Predicate::eq("value", 30i64),
            ]),
        ),
        "where_not" => node_filter(rt, client, Predicate::not(Predicate::gt("value", 50i64))),
        "where_compound_and_or" => node_filter(
            rt,
            client,
            Predicate::and(vec![
                Predicate::gt("value", 25i64),
                Predicate::or(vec![
                    Predicate::lt("value", 50i64),
                    Predicate::lt("id", 100i64),
                ]),
            ]),
        ),
        "where_starts_with" => node_filter(rt, client, Predicate::starts_with("name", "node_5")),
        "where_ends_with" => node_filter(rt, client, Predicate::ends_with("name", "0")),
        "where_contains" => node_filter(rt, client, Predicate::contains("name", "99")),
        "where_modulo_eq" => node_filter(
            rt,
            client,
            Predicate::compare(
                Expr::prop("id").modulo(Expr::val(7i64)),
                CompareOp::Eq,
                Expr::val(0i64),
            ),
        ),
        "where_string_gte" => node_filter(rt, client, Predicate::gte("name", "node_500")),
        "where_two_props" => node_filter(
            rt,
            client,
            Predicate::and(vec![
                Predicate::gt("value", 50i64),
                Predicate::lt("id", 100i64),
            ]),
        ),
        "where_id_in_range" => node_filter(
            rt,
            client,
            Predicate::and(vec![
                Predicate::gte("id", 100i64),
                Predicate::lte("id", 200i64),
            ]),
        ),
        "where_subexpr" => node_filter(
            rt,
            client,
            Predicate::compare(
                Expr::prop("value")
                    .add(Expr::prop("id"))
                    .modulo(Expr::val(2i64)),
                CompareOp::Eq,
                Expr::val(0i64),
            ),
        ),
        "with_two_chained" => node_filter(
            rt,
            client,
            Predicate::and(vec![
                Predicate::gt("value", 25i64),
                Predicate::lt("value", 75i64),
            ]),
        ),
        // ---- computed projections: server-side `project` with Expr ----
        "numeric_modulo" => node_project(rt, client, Expr::prop("value").modulo(Expr::val(13i64))),
        "numeric_pow" => node_project(rt, client, Expr::prop("value").mul(Expr::prop("value"))),
        "computed_in_return" => node_project(rt, client, Expr::prop("value").mul(Expr::val(2i64))),
        "case_when" => node_project(
            rt,
            client,
            Expr::case(
                vec![(Predicate::gt("value", 50i64), Expr::val("high"))],
                Some(Expr::val("low")),
            ),
        ),
        "coalesce_existing" => node_project(
            rt,
            client,
            Expr::case(
                vec![(Predicate::is_null("value"), Expr::val(0i64))],
                Some(Expr::prop("value")),
            ),
        ),
        // ---- aggregates (server-side aggregate_by) ----
        "aggregate_sum" => node_aggregate(rt, client, AggregateFunction::Sum),
        "aggregate_min" => node_aggregate(rt, client, AggregateFunction::Min),
        "aggregate_max" => node_aggregate(rt, client, AggregateFunction::Max),
        "aggregate_avg" => node_aggregate(rt, client, AggregateFunction::Mean),
        // ---- sort & paginate ----
        "order_by_id_asc" => q_order_by_id(rt, client),
        "order_by_multi_key" => q_order_by_multi(rt, client),
        "skip_limit" => q_skip_limit(rt, client),
        "list_in_construction" => q_list_in_construction(rt, client),
        // ---- traversals ----
        "one_hop" => q_one_hop(rt, client),
        "two_hop" => q_two_hop(rt, client),
        "three_hop" => q_three_hop(rt, client),
        "filter_one_hop" => q_filter_one_hop(rt, client),
        "reverse" => q_reverse(rt, client),
        "undirected" => q_undirected(rt, client),
        "direct_record" => q_direct_record(rt, client),
        "relation_filter" => {
            q_relation_filter(rt, client, arg.expect("relation_filter: missing arg"))
        }
        "recursive_depth2" => q_recursive_depth(rt, client, 2),
        "recursive_depth3" => q_recursive_depth(rt, client, 3),
        "recursive_depth5" => q_recursive_depth(rt, client, 5),
        // ---- patterns ----
        "star_fanout" => q_star_fanout(rt, client),
        "star_fanout_filter" => q_star_fanout_filter(rt, client),
        "edge_subquery" => q_edge_subquery(rt, client),
        // ---- writes ----
        "write_single" => q_write_single(rt, client),
        "update_set" => q_update_set(rt, client, arg.expect("update_set: missing arg")),
        "set_multiple_props" => {
            q_set_multiple(rt, client, arg.expect("set_multiple_props: missing arg"))
        }
        "bulk_set_match" => q_bulk_set_match(rt, client),
        "delete_node" => q_delete_node(rt, client, arg.expect("delete_node: missing arg")),
        "bulk_edges" => q_bulk_edges(rt, client, arg.expect("bulk_edges: missing arg") as usize),
        // count()-based: kept for servers without the count() rate limit
        // (the YAML notes these out on the enterprise-dev image).
        "star_fanout_count" => q_star_fanout_count(rt, client),
        "aggregate_count" => q_aggregate_count(rt, client),
        other => panic!("helixdb execute: unknown sentinel keyword `{other}` (full query: `{q}`)"),
    }
}

fn q_scan_label(rt: &Runtime, client: &Client) {
    let req = DynamicQueryRequest::read(
        read_batch()
            .var_as("rows", g().n_with_label("Node").values(vec!["id"]))
            .returning(["rows"]),
    );
    let _: Ignored = run_read(rt, client, req);
}

fn q_scan_filtered(rt: &Runtime, client: &Client) {
    let req = DynamicQueryRequest::read(
        read_batch()
            .var_as(
                "rows",
                g().n_with_label_where("Node", SourcePredicate::gt("value", 50i64))
                    .values(vec!["id"]),
            )
            .returning(["rows"]),
    );
    let _: Ignored = run_read(rt, client, req);
}

fn q_lookup_by_id(rt: &Runtime, client: &Client, id: i64) {
    let req = DynamicQueryRequest::read(
        read_batch()
            .var_as(
                "rows",
                g().n_with_label_where("Node", SourcePredicate::eq("id", id))
                    .values(vec!["name"]),
            )
            .returning(["rows"]),
    );
    let _: Ignored = run_read(rt, client, req);
}

fn q_one_hop(rt: &Runtime, client: &Client) {
    let req = DynamicQueryRequest::read(
        read_batch()
            .var_as(
                "rows",
                g().n_with_label("Chain")
                    .out(Some("NEXT"))
                    .values(vec!["idx"]),
            )
            .returning(["rows"]),
    );
    let _: Ignored = run_read(rt, client, req);
}

fn q_two_hop(rt: &Runtime, client: &Client) {
    let req = DynamicQueryRequest::read(
        read_batch()
            .var_as(
                "rows",
                g().n_with_label("Chain")
                    .out(Some("NEXT"))
                    .out(Some("NEXT"))
                    .values(vec!["idx"]),
            )
            .returning(["rows"]),
    );
    let _: Ignored = run_read(rt, client, req);
}

fn q_three_hop(rt: &Runtime, client: &Client) {
    // `q_two_hop` with one more `out(NEXT)` step.
    let req = DynamicQueryRequest::read(
        read_batch()
            .var_as(
                "rows",
                g().n_with_label("Chain")
                    .out(Some("NEXT"))
                    .out(Some("NEXT"))
                    .out(Some("NEXT"))
                    .values(vec!["idx"]),
            )
            .returning(["rows"]),
    );
    let _: Ignored = run_read(rt, client, req);
}

fn q_filter_one_hop(rt: &Runtime, client: &Client) {
    // (Chain WHERE idx > 100)-[:NEXT]->(Chain). The source-side predicate
    // reuses the `gt` path exercised by `q_scan_filtered`.
    let req = DynamicQueryRequest::read(
        read_batch()
            .var_as(
                "rows",
                g().n_with_label_where("Chain", SourcePredicate::gt("idx", 100i64))
                    .out(Some("NEXT"))
                    .values(vec!["idx"]),
            )
            .returning(["rows"]),
    );
    let _: Ignored = run_read(rt, client, req);
}

fn q_star_fanout(rt: &Runtime, client: &Client) {
    let req = DynamicQueryRequest::read(
        read_batch()
            .var_as(
                "rows",
                g().n_with_label("Hub").out(Some("ARM")).values(vec!["id"]),
            )
            .returning(["rows"]),
    );
    let _: Ignored = run_read(rt, client, req);
}

fn q_star_fanout_count(rt: &Runtime, client: &Client) {
    // Leaf count for the star fixture: `q_star_fanout`'s traversal capped
    // with the `count()` terminal `q_aggregate_count` uses on a node stream.
    let req = DynamicQueryRequest::read(
        read_batch()
            .var_as("c", g().n_with_label("Hub").out(Some("ARM")).count())
            .returning(["c"]),
    );
    let _: Ignored = run_read(rt, client, req);
}

fn q_aggregate_count(rt: &Runtime, client: &Client) {
    let req = DynamicQueryRequest::read(
        read_batch()
            .var_as("c", g().n_with_label("Node").count())
            .returning(["c"]),
    );
    let _: Ignored = run_read(rt, client, req);
}

// ---------------------------------------------------------------------------
// Shared bodies: predicate scans, computed projections, aggregates. The
// work is pushed down to HelixDB's DSL (no client-side folding).
// ---------------------------------------------------------------------------

/// `MATCH (n:Node) WHERE <pred> RETURN n.id` — server-side `where_`.
fn node_filter(rt: &Runtime, client: &Client, pred: Predicate) {
    let req = DynamicQueryRequest::read(
        read_batch()
            .var_as(
                "rows",
                g().n_with_label("Node").where_(pred).values(vec!["id"]),
            )
            .returning(["rows"]),
    );
    let _: Ignored = run_read(rt, client, req);
}

/// `MATCH (n:Node) RETURN <expr>` — server-side computed `project`.
fn node_project(rt: &Runtime, client: &Client, expr: Expr) {
    let req = DynamicQueryRequest::read(
        read_batch()
            .var_as(
                "rows",
                g().n_with_label("Node")
                    .project(vec![Projection::expr("r", expr)]),
            )
            .returning(["rows"]),
    );
    let _: Ignored = run_read(rt, client, req);
}

/// `MATCH (n:Node) RETURN <agg>(n.value)` — server-side `aggregate_by`.
fn node_aggregate(rt: &Runtime, client: &Client, f: AggregateFunction) {
    let req = DynamicQueryRequest::read(
        read_batch()
            .var_as("agg", g().n_with_label("Node").aggregate_by(f, "value"))
            .returning(["agg"]),
    );
    let _: Ignored = run_read(rt, client, req);
}

// ---------------------------------------------------------------------------
// sort & paginate
// ---------------------------------------------------------------------------

fn q_order_by_id(rt: &Runtime, client: &Client) {
    let req = DynamicQueryRequest::read(
        read_batch()
            .var_as(
                "rows",
                g().n_with_label("Node")
                    .order_by("id", Order::Asc)
                    .limit(100i64)
                    .values(vec!["id"]),
            )
            .returning(["rows"]),
    );
    let _: Ignored = run_read(rt, client, req);
}

fn q_order_by_multi(rt: &Runtime, client: &Client) {
    let req = DynamicQueryRequest::read(
        read_batch()
            .var_as(
                "rows",
                g().n_with_label("Node")
                    .order_by_multiple(vec![("value", Order::Desc), ("id", Order::Asc)])
                    .limit(50i64)
                    .values(vec!["id"]),
            )
            .returning(["rows"]),
    );
    let _: Ignored = run_read(rt, client, req);
}

fn q_skip_limit(rt: &Runtime, client: &Client) {
    let req = DynamicQueryRequest::read(
        read_batch()
            .var_as(
                "rows",
                g().n_with_label("Node")
                    .order_by("id", Order::Asc)
                    .skip(100i64)
                    .limit(50i64)
                    .values(vec!["id"]),
            )
            .returning(["rows"]),
    );
    let _: Ignored = run_read(rt, client, req);
}

fn q_list_in_construction(rt: &Runtime, client: &Client) {
    // RETURN [n.id, n.value] — project both values per row.
    let req = DynamicQueryRequest::read(
        read_batch()
            .var_as("rows", g().n_with_label("Node").values(vec!["id", "value"]))
            .returning(["rows"]),
    );
    let _: Ignored = run_read(rt, client, req);
}

// ---------------------------------------------------------------------------
// remaining traversals
// ---------------------------------------------------------------------------

fn q_reverse(rt: &Runtime, client: &Client) {
    // (a:Chain)<-[:NEXT]-(b): incoming edge traversal.
    let req = DynamicQueryRequest::read(
        read_batch()
            .var_as(
                "rows",
                g().n_with_label("Chain")
                    .in_(Some("NEXT"))
                    .values(vec!["idx"]),
            )
            .returning(["rows"]),
    );
    let _: Ignored = run_read(rt, client, req);
}

fn q_undirected(rt: &Runtime, client: &Client) {
    // (a:Chain)-[:NEXT]-(b): both directions.
    let req = DynamicQueryRequest::read(
        read_batch()
            .var_as(
                "rows",
                g().n_with_label("Chain")
                    .both(Some("NEXT"))
                    .values(vec!["idx"]),
            )
            .returning(["rows"]),
    );
    let _: Ignored = run_read(rt, client, req);
}

fn q_direct_record(rt: &Runtime, client: &Client) {
    // (a:Chain {idx: 0})-[:NEXT]->(b) RETURN b.idx.
    let req = DynamicQueryRequest::read(
        read_batch()
            .var_as(
                "rows",
                g().n_with_label_where("Chain", SourcePredicate::eq("idx", 0i64))
                    .out(Some("NEXT"))
                    .values(vec!["idx"]),
            )
            .returning(["rows"]),
    );
    let _: Ignored = run_read(rt, client, req);
}

fn q_relation_filter(rt: &Runtime, client: &Client, half: i64) {
    // (a)-[r:NEXT]->(b) WHERE r.step >= half. Scan NEXT edges with the
    // edge-property predicate pushed down via `e_with_label_where`
    // (`where_` is node-only; `edge_has` is equality-only), then step to
    // the target node.
    let req = DynamicQueryRequest::read(
        read_batch()
            .var_as(
                "rows",
                g().e_with_label_where("NEXT", SourcePredicate::gte("step", half))
                    .in_n()
                    .values(vec!["idx"]),
            )
            .returning(["rows"]),
    );
    let _: Ignored = run_read(rt, client, req);
}

fn q_recursive_depth(rt: &Runtime, client: &Client, k: usize) {
    // Fixed-depth expansion from Chain idx 0: `k` chained `out(NEXT)` hops.
    let mut trav = g()
        .n_with_label_where("Chain", SourcePredicate::eq("idx", 0i64))
        .out(Some("NEXT"));
    for _ in 1..k {
        trav = trav.out(Some("NEXT"));
    }
    let req = DynamicQueryRequest::read(
        read_batch()
            .var_as("rows", trav.values(vec!["idx"]))
            .returning(["rows"]),
    );
    let _: Ignored = run_read(rt, client, req);
}

// ---------------------------------------------------------------------------
// remaining patterns
// ---------------------------------------------------------------------------

fn q_star_fanout_filter(rt: &Runtime, client: &Client) {
    // (h:Hub)-[:ARM]->(l:Leaf) WHERE l.id < 100.
    let req = DynamicQueryRequest::read(
        read_batch()
            .var_as(
                "rows",
                g().n_with_label("Hub")
                    .out(Some("ARM"))
                    .where_(Predicate::lt("id", 100i64))
                    .values(vec!["id"]),
            )
            .returning(["rows"]),
    );
    let _: Ignored = run_read(rt, client, req);
}

fn q_edge_subquery(rt: &Runtime, client: &Client) {
    // (p:Person)-[k:KNOWS]->(f) WHERE k.strength >= 3 — edge-property
    // predicate pushed down via `e_with_label_where`, then to the target.
    let req = DynamicQueryRequest::read(
        read_batch()
            .var_as(
                "rows",
                g().e_with_label_where("KNOWS", SourcePredicate::gte("strength", 3i64))
                    .in_n()
                    .values(vec!["idx"]),
            )
            .returning(["rows"]),
    );
    let _: Ignored = run_read(rt, client, req);
}

// ---------------------------------------------------------------------------
// writes
// ---------------------------------------------------------------------------

fn q_write_single(rt: &Runtime, client: &Client) {
    let req = DynamicQueryRequest::write(
        write_batch()
            .var_as(
                "n",
                g().add_n(
                    "N",
                    vec![
                        ("id", PropertyValue::from(1i64)),
                        ("name", PropertyValue::from("a")),
                        ("value", PropertyValue::from(42i64)),
                    ],
                ),
            )
            .returning(["n"]),
    );
    run_write(rt, client, req);
}

fn q_update_set(rt: &Runtime, client: &Client, half: i64) {
    let req = DynamicQueryRequest::write(
        write_batch()
            .var_as(
                "u",
                g().n_with_label_where("Node", SourcePredicate::eq("id", half))
                    .set_property("touched", true),
            )
            .returning(["u"]),
    );
    run_write(rt, client, req);
}

fn q_set_multiple(rt: &Runtime, client: &Client, half: i64) {
    let req = DynamicQueryRequest::write(
        write_batch()
            .var_as(
                "u",
                g().n_with_label_where("Node", SourcePredicate::eq("id", half))
                    .set_property("a", 1i64)
                    .set_property("b", 2i64),
            )
            .returning(["u"]),
    );
    run_write(rt, client, req);
}

fn q_bulk_set_match(rt: &Runtime, client: &Client) {
    let req = DynamicQueryRequest::write(
        write_batch()
            .var_as(
                "u",
                g().n_with_label_where("Node", SourcePredicate::gt("value", 50i64))
                    .set_property("flagged", true),
            )
            .returning(["u"]),
    );
    run_write(rt, client, req);
}

fn q_delete_node(rt: &Runtime, client: &Client, half: i64) {
    let req = DynamicQueryRequest::write(
        write_batch()
            .var_as(
                "d",
                g().n_with_label_where("Node", SourcePredicate::eq("id", half))
                    .drop(),
            )
            .returning(["d"]),
    );
    run_write(rt, client, req);
}

fn q_bulk_edges(rt: &Runtime, client: &Client, size: usize) {
    // UNWIND range CREATE (a {id:i})-[:LINKS]->(b {id:(i+1)%size}) — built as
    // one write batch of per-edge var bindings, like `seed_chain`'s edges.
    let mut batch = write_batch();
    let mut names: Vec<String> = Vec::with_capacity(size);
    for k in 0..size {
        let other = (k + 1) % size;
        let src = format!("s{k}");
        let dst = format!("d{k}");
        let edge = format!("le{k}");
        batch = batch
            .var_as(
                &src,
                g().n_with_label_where("Node", SourcePredicate::eq("id", k as i64)),
            )
            .var_as(
                &dst,
                g().n_with_label_where("Node", SourcePredicate::eq("id", other as i64)),
            )
            .var_as(
                &edge,
                g().n(NodeRef::var(src)).add_e(
                    "LINKS",
                    NodeRef::var(dst),
                    Vec::<(&str, PropertyValue)>::new(),
                ),
            );
        names.push(edge);
    }
    let req = DynamicQueryRequest::write(batch.returning(names));
    run_write(rt, client, req);
}

// ---------------------------------------------------------------------------
// Seeding
// ---------------------------------------------------------------------------

fn seed_nodes(rt: &Runtime, client: &Client, n: usize) {
    let mut i = 0;
    while i < n {
        let end = (i + CHUNK).min(n);
        let mut batch = write_batch();
        let mut names: Vec<String> = Vec::with_capacity(end - i);
        for k in i..end {
            let var = format!("n{k}");
            batch = batch.var_as(
                &var,
                g().add_n(
                    "Node",
                    vec![
                        ("id", PropertyValue::from(k as i64)),
                        ("name", PropertyValue::from(format!("node_{k}"))),
                        ("value", PropertyValue::from((k % 100) as i64)),
                    ],
                ),
            );
            names.push(var);
        }
        let req = DynamicQueryRequest::write(batch.returning(names));
        run_write(rt, client, req);
        i = end;
    }
}

fn seed_chain(rt: &Runtime, client: &Client, len: usize) {
    // Two passes: nodes first (so we know their freshly-allocated IDs by
    // selecting on `idx`), then edges. Building both in one batch is
    // possible but the cartesian-product semantics of `add_e` on a node
    // stream require carrying every per-row variable through `var_as`,
    // which the DSL doesn't expose cleanly. Two passes is simpler and
    // matches how the surrealdb adapter seeds.
    let mut i = 0;
    while i < len {
        let end = (i + CHUNK).min(len);
        let mut batch = write_batch();
        let mut names: Vec<String> = Vec::with_capacity(end - i);
        for k in i..end {
            let var = format!("c{k}");
            batch = batch.var_as(
                &var,
                g().add_n("Chain", vec![("idx", PropertyValue::from(k as i64))]),
            );
            names.push(var);
        }
        let req = DynamicQueryRequest::write(batch.returning(names));
        run_write(rt, client, req);
        i = end;
    }
    if len > 1 {
        let mut i = 0;
        while i < len - 1 {
            let end = (i + CHUNK).min(len - 1);
            let mut batch = write_batch();
            let mut names: Vec<String> = Vec::with_capacity(end - i);
            for k in i..end {
                let src_var = format!("s{k}");
                let dst_var = format!("d{k}");
                let edge_var = format!("e{k}");
                batch = batch
                    .var_as(
                        &src_var,
                        g().n_with_label_where("Chain", SourcePredicate::eq("idx", k as i64)),
                    )
                    .var_as(
                        &dst_var,
                        g().n_with_label_where("Chain", SourcePredicate::eq("idx", (k + 1) as i64)),
                    )
                    .var_as(
                        &edge_var,
                        g().n(NodeRef::var(src_var)).add_e(
                            "NEXT",
                            NodeRef::var(dst_var),
                            vec![("step", PropertyValue::from(k as i64))],
                        ),
                    );
                names.push(edge_var);
            }
            let req = DynamicQueryRequest::write(batch.returning(names));
            run_write(rt, client, req);
            i = end;
        }
    }
}

fn seed_star(rt: &Runtime, client: &Client, spokes: usize) {
    // Hub first, on its own — keeps the per-leaf batches independent of
    // the hub's allocated ID.
    let req = DynamicQueryRequest::write(
        write_batch()
            .var_as(
                "hub",
                g().add_n("Hub", vec![("name", PropertyValue::from("center"))]),
            )
            .returning(["hub"]),
    );
    run_write(rt, client, req);

    let mut i = 0;
    while i < spokes {
        let end = (i + CHUNK).min(spokes);
        let mut batch = write_batch();
        let mut names: Vec<String> = Vec::with_capacity(2 * (end - i));
        for k in i..end {
            let leaf_var = format!("l{k}");
            let edge_var = format!("a{k}");
            batch = batch
                .var_as(
                    &leaf_var,
                    g().add_n("Leaf", vec![("id", PropertyValue::from(k as i64))]),
                )
                .var_as(
                    &edge_var,
                    g().n_with_label_where("Hub", SourcePredicate::eq("name", "center"))
                        .add_e(
                            "ARM",
                            NodeRef::var(leaf_var),
                            Vec::<(&str, PropertyValue)>::new(),
                        ),
                );
            names.push(edge_var);
        }
        let req = DynamicQueryRequest::write(batch.returning(names));
        run_write(rt, client, req);
        i = end;
    }
}

fn seed_social(rt: &Runtime, client: &Client, n: usize) {
    // People first.
    let mut i = 0;
    while i < n {
        let end = (i + CHUNK).min(n);
        let mut batch = write_batch();
        let mut names: Vec<String> = Vec::with_capacity(end - i);
        for k in i..end {
            let var = format!("p{k}");
            batch = batch.var_as(
                &var,
                g().add_n(
                    "Person",
                    vec![
                        ("idx", PropertyValue::from(k as i64)),
                        ("name", PropertyValue::from(format!("person_{k}"))),
                    ],
                ),
            );
            names.push(var);
        }
        let req = DynamicQueryRequest::write(batch.returning(names));
        run_write(rt, client, req);
        i = end;
    }
    // Two outgoing edges per person, matching the other adapters.
    for offset in 1..=2usize {
        let mut i = 0;
        while i < n {
            let end = (i + CHUNK).min(n);
            let mut batch = write_batch();
            let mut names: Vec<String> = Vec::with_capacity(end - i);
            for k in i..end {
                let other = (k + offset) % n;
                let strength = ((k + offset) % 5) as i64;
                let src_var = format!("s{offset}_{k}");
                let dst_var = format!("d{offset}_{k}");
                let edge_var = format!("e{offset}_{k}");
                batch = batch
                    .var_as(
                        &src_var,
                        g().n_with_label_where("Person", SourcePredicate::eq("idx", k as i64)),
                    )
                    .var_as(
                        &dst_var,
                        g().n_with_label_where("Person", SourcePredicate::eq("idx", other as i64)),
                    )
                    .var_as(
                        &edge_var,
                        g().n(NodeRef::var(src_var)).add_e(
                            "KNOWS",
                            NodeRef::var(dst_var),
                            vec![("strength", PropertyValue::from(strength))],
                        ),
                    );
                names.push(edge_var);
            }
            let req = DynamicQueryRequest::write(batch.returning(names));
            run_write(rt, client, req);
            i = end;
        }
    }
}
