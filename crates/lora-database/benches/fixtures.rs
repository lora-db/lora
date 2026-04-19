#![allow(dead_code)]
//! Reusable graph fixture builders for benchmarks.
//!
//! Each builder returns a fully populated `Database<InMemoryGraph>` ready
//! for read-only benchmarking.  Graphs are constructed via Lora statements,
//! so the cost of creation is realistic but should be paid **once** outside
//! the hot loop (use `criterion::Bencher::iter_batched` with `Setup::PerIteration`
//! only when measuring write throughput).
//!
//! ## Graph builders
//!
//! | Builder                        | Labels / Rels                        | Purpose                          |
//! |-------------------------------|--------------------------------------|----------------------------------|
//! | `build_node_graph(n)`         | `:Node`                              | Isolated nodes with properties   |
//! | `build_chain(len)`            | `:Chain` / `:NEXT`                   | Linear chain                     |
//! | `build_cycle(len)`            | `:Ring`  / `:LOOP`                   | Cyclic graph                     |
//! | `build_star(spokes)`          | `:Hub`, `:Leaf` / `:ARM`             | Star fan-out                     |
//! | `build_social_graph(n, k)`    | `:Person` / `:KNOWS`                 | Social network                   |
//! | `build_tree(d, b)`            | `:Tree`  / `:CHILD`                  | N-ary tree                       |
//! | `build_dependency_graph(n)`   | `:Package` / `:DEPENDS_ON`           | DAG                              |
//! | `build_org_graph()`           | multiple / multiple                  | Fixed 12-node org chart          |
//! | `build_temporal_graph(n)`     | `:Event` / `:FOLLOWS`                | Events with date/time properties |
//! | `build_spatial_graph(n)`      | `:Location` / `:CONNECTS_TO`         | Locations with point coords      |
//! | `build_recommendation_graph`  | `:User`, `:Product` / `:BOUGHT`, etc | Bipartite user-product graph     |

use std::collections::BTreeMap;

use lora_database::{Database, ExecuteOptions, InMemoryGraph, LoraValue, ResultFormat};

// ---------------------------------------------------------------------------
// Wrapper (mirrors TestDb from integration tests but without serde_json dep)
// ---------------------------------------------------------------------------

pub struct BenchDb {
    pub service: Database<InMemoryGraph>,
}

impl Default for BenchDb {
    fn default() -> Self {
        Self::new()
    }
}

impl BenchDb {
    pub fn new() -> Self {
        Self {
            service: Database::in_memory(),
        }
    }

    /// Execute a Lora statement.  Panics on error — fine for setup code.
    pub fn run(&self, cypher: &str) {
        let options = Some(ExecuteOptions {
            format: ResultFormat::Rows,
        });
        self.service
            .execute(cypher, options)
            .unwrap_or_else(|e| panic!("bench setup failed: {cypher}\nerror: {e}"));
    }

    /// Execute with parameters.
    pub fn run_with_params(&self, cypher: &str, params: BTreeMap<String, LoraValue>) {
        let options = Some(ExecuteOptions {
            format: ResultFormat::Rows,
        });
        self.service
            .execute_with_params(cypher, options, params)
            .unwrap_or_else(|e| panic!("bench setup failed: {cypher}\nerror: {e}"));
    }
}

// ---------------------------------------------------------------------------
// Scale parameters
// ---------------------------------------------------------------------------

/// Predefined scale levels for parametric benchmarks.
pub struct Scale;

impl Scale {
    pub const TINY: usize = 100;
    pub const SMALL: usize = 1_000;
    pub const MEDIUM: usize = 10_000;
    // LARGE was 100k; halved to keep O(n) / O(n log n) signal while cutting
    // build + per-iter cost roughly in half for the scale_* groups.
    pub const LARGE: usize = 50_000;
}

// UNWIND batch size used by bulk node builders. Bigger batches mean fewer
// parse/compile cycles during fixture setup, which dominates large builds.
const BULK_BATCH: usize = 2_000;
// Builders that produce verbose per-row CASE/temporal expressions use a
// smaller batch to keep the compiled plan manageable.
const RICH_BATCH: usize = 500;

// ---------------------------------------------------------------------------
// Generic graph builders
// ---------------------------------------------------------------------------

/// Create a graph with `n` isolated nodes, each labelled `:Node` with
/// properties `{id: <i>, name: 'node_<i>', value: <i % 100>}`.
pub fn build_node_graph(n: usize) -> BenchDb {
    let db = BenchDb::new();
    // Use UNWIND for bulk creation — much faster than individual CREATEs.
    let batch = BULK_BATCH;
    let mut i = 0usize;
    while i < n {
        let end = (i + batch).min(n);
        db.run(&format!(
            "UNWIND range({i}, {}) AS i CREATE (:Node {{id: i, name: 'node_' + toString(i), value: i % 100}})",
            end - 1
        ));
        i = end;
    }
    db
}

/// Create a linear chain: n0 -> n1 -> … -> n(len-1) with `:NEXT` edges.
pub fn build_chain(len: usize) -> BenchDb {
    let db = BenchDb::new();
    let batch = BULK_BATCH;
    let mut i = 0usize;
    while i < len {
        let end = (i + batch).min(len);
        db.run(&format!(
            "UNWIND range({i}, {}) AS i CREATE (:Chain {{idx: i}})",
            end - 1
        ));
        i = end;
    }
    // Create edges in batches
    if len > 1 {
        let mut i = 0usize;
        while i < len - 1 {
            let end = (i + batch).min(len - 1);
            db.run(&format!(
                "UNWIND range({i}, {}) AS i \
                 MATCH (a:Chain {{idx: i}}), (b:Chain {{idx: i + 1}}) \
                 CREATE (a)-[:NEXT]->(b)",
                end - 1
            ));
            i = end;
        }
    }
    db
}

/// Create a cycle: n0 -> n1 -> … -> n(len-1) -> n0 with `:LOOP` edges.
pub fn build_cycle(len: usize) -> BenchDb {
    let db = BenchDb::new();
    let batch = BULK_BATCH;
    let mut i = 0usize;
    while i < len {
        let end = (i + batch).min(len);
        db.run(&format!(
            "UNWIND range({i}, {}) AS i CREATE (:Ring {{idx: i}})",
            end - 1
        ));
        i = end;
    }
    // forward edges
    if len > 1 {
        let mut i = 0usize;
        while i < len - 1 {
            let end = (i + batch).min(len - 1);
            db.run(&format!(
                "UNWIND range({i}, {}) AS i \
                 MATCH (a:Ring {{idx: i}}), (b:Ring {{idx: i + 1}}) \
                 CREATE (a)-[:LOOP]->(b)",
                end - 1
            ));
            i = end;
        }
    }
    // closing edge
    if len > 1 {
        db.run(&format!(
            "MATCH (a:Ring {{idx: {}}}), (b:Ring {{idx: 0}}) CREATE (a)-[:LOOP]->(b)",
            len - 1
        ));
    }
    db
}

/// Build a star graph: one `:Hub` node with `spokes` outgoing `:ARM` edges
/// to `:Leaf` nodes.
pub fn build_star(spokes: usize) -> BenchDb {
    let db = BenchDb::new();
    db.run("CREATE (:Hub {name: 'center'})");
    let batch = BULK_BATCH;
    let mut i = 0usize;
    while i < spokes {
        let end = (i + batch).min(spokes);
        db.run(&format!(
            "UNWIND range({i}, {}) AS i \
             MATCH (h:Hub) CREATE (h)-[:ARM]->(:Leaf {{id: i}})",
            end - 1
        ));
        i = end;
    }
    db
}

/// Build a social network graph.
///
/// * `n` people with `:Person` label
/// * Each person connects to `avg_friends` others via `:KNOWS`
/// * Properties: `{id, name, age, city}`
/// * 5 distinct cities, age 20-60
pub fn build_social_graph(n: usize, avg_friends: usize) -> BenchDb {
    let db = BenchDb::new();
    let cities = ["London", "Berlin", "Paris", "Tokyo", "Amsterdam"];

    // Create people
    let batch = BULK_BATCH;
    let mut i = 0usize;
    while i < n {
        let end = (i + batch).min(n);
        db.run(&format!(
            "UNWIND range({i}, {}) AS i \
             CREATE (:Person {{id: i, name: 'person_' + toString(i), age: 20 + (i % 41), city: CASE i % 5 \
                WHEN 0 THEN '{c0}' WHEN 1 THEN '{c1}' WHEN 2 THEN '{c2}' \
                WHEN 3 THEN '{c3}' ELSE '{c4}' END}})",
            end - 1,
            c0 = cities[0],
            c1 = cities[1],
            c2 = cities[2],
            c3 = cities[3],
            c4 = cities[4],
        ));
        i = end;
    }

    // Create KNOWS relationships (deterministic, not random)
    // Each person i connects to persons (i+1)%n, (i+2)%n, … (i+avg_friends)%n
    if n > 1 {
        let mut i = 0usize;
        while i < n {
            let end = (i + batch).min(n);
            for j in 1..=avg_friends.min(n - 1) {
                db.run(&format!(
                    "UNWIND range({i}, {}) AS i \
                     MATCH (a:Person {{id: i}}), (b:Person {{id: (i + {j}) % {n}}}) \
                     CREATE (a)-[:KNOWS {{strength: (i + {j}) % 10}}]->(b)",
                    end - 1,
                ));
            }
            i = end;
        }
    }
    db
}

/// Build a tree graph of given depth and branching factor.
///
/// Total nodes = (branch^(depth+1) - 1) / (branch - 1) for branch > 1.
/// Labels: `:Tree`, relationships: `:CHILD`.
pub fn build_tree(depth: usize, branch: usize) -> BenchDb {
    let db = BenchDb::new();
    db.run("CREATE (:Tree {id: 0, depth: 0})");
    let mut next_id = 1u64;
    let mut current_ids: Vec<u64> = vec![0];

    for d in 0..depth {
        let mut new_ids = Vec::new();
        for &parent_id in &current_ids {
            for _ in 0..branch {
                let child_id = next_id;
                next_id += 1;
                db.run(&format!(
                    "MATCH (p:Tree {{id: {parent_id}}}) \
                     CREATE (p)-[:CHILD]->(:Tree {{id: {child_id}, depth: {}}})",
                    d + 1
                ));
                new_ids.push(child_id);
            }
        }
        current_ids = new_ids;
    }
    db
}

/// Build a dependency-style DAG: `n` packages, each depending on 1-3 others
/// (deterministic). Labels: `:Package`, relationships: `:DEPENDS_ON`.
pub fn build_dependency_graph(n: usize) -> BenchDb {
    let db = BenchDb::new();
    let batch = BULK_BATCH;
    let mut i = 0usize;
    while i < n {
        let end = (i + batch).min(n);
        db.run(&format!(
            "UNWIND range({i}, {}) AS i \
             CREATE (:Package {{id: i, name: 'pkg_' + toString(i), version: '1.' + toString(i % 10)}})",
            end - 1
        ));
        i = end;
    }
    // Each package i depends on packages at lower ids
    if n > 1 {
        for i in 1..n {
            // Depend on 1-3 predecessors
            let deps = (i % 3) + 1;
            for d in 1..=deps {
                let target = i.saturating_sub(d);
                if target < i {
                    db.run(&format!(
                        "MATCH (a:Package {{id: {i}}}), (b:Package {{id: {target}}}) \
                         CREATE (a)-[:DEPENDS_ON]->(b)"
                    ));
                }
            }
        }
    }
    db
}

// ---------------------------------------------------------------------------
// Pre-seeded fixture graphs (small, for warm-up and comparison)
// ---------------------------------------------------------------------------

/// The org-chart graph from integration tests (12 nodes, 20 relationships).
pub fn build_org_graph() -> BenchDb {
    let db = BenchDb::new();
    // Nodes
    db.run("CREATE (:Person {name:'Alice', age:35, dept:'Engineering'})");
    db.run("CREATE (:Person {name:'Bob',   age:28, dept:'Engineering'})");
    db.run("CREATE (:Person {name:'Carol', age:42, dept:'Marketing'})");
    db.run("CREATE (:Person {name:'Dave',  age:31, dept:'Marketing'})");
    db.run("CREATE (:Person {name:'Eve',   age:26, dept:'Engineering'})");
    db.run("CREATE (:Person:Manager {name:'Frank', age:50, dept:'Engineering'})");
    db.run("CREATE (:Company {name:'Acme', founded: 2010})");
    db.run("CREATE (:Project {name:'Alpha', budget: 100000})");
    db.run("CREATE (:Project {name:'Beta',  budget: 50000})");
    db.run("CREATE (:City {name:'London'})");
    db.run("CREATE (:City {name:'Berlin'})");
    db.run("CREATE (:City {name:'Tokyo'})");
    for (person, since) in [
        ("Alice", 2018),
        ("Bob", 2020),
        ("Carol", 2015),
        ("Dave", 2021),
        ("Eve", 2022),
        ("Frank", 2012),
    ] {
        db.run(&format!(
            "MATCH (p:Person {{name:'{person}'}}), (c:Company {{name:'Acme'}}) \
             CREATE (p)-[:WORKS_AT {{since:{since}}}]->(c)"
        ));
    }
    for (mgr, sub) in [
        ("Frank", "Alice"),
        ("Frank", "Bob"),
        ("Frank", "Eve"),
        ("Carol", "Dave"),
    ] {
        db.run(&format!(
            "MATCH (m:Person {{name:'{mgr}'}}), (s:Person {{name:'{sub}'}}) \
             CREATE (m)-[:MANAGES]->(s)"
        ));
    }
    for (person, project, role) in [
        ("Alice", "Alpha", "lead"),
        ("Bob", "Alpha", "dev"),
        ("Carol", "Beta", "lead"),
        ("Eve", "Beta", "dev"),
    ] {
        db.run(&format!(
            "MATCH (p:Person {{name:'{person}'}}), (pr:Project {{name:'{project}'}}) \
             CREATE (p)-[:ASSIGNED_TO {{role:'{role}'}}]->(pr)"
        ));
    }
    for (person, city) in [
        ("Alice", "London"),
        ("Bob", "Berlin"),
        ("Carol", "London"),
        ("Dave", "Tokyo"),
        ("Eve", "Berlin"),
        ("Frank", "London"),
    ] {
        db.run(&format!(
            "MATCH (p:Person {{name:'{person}'}}), (c:City {{name:'{city}'}}) \
             CREATE (p)-[:LIVES_IN]->(c)"
        ));
    }
    db
}

// ---------------------------------------------------------------------------
// Temporal graph builder
// ---------------------------------------------------------------------------

/// Build a graph of `:Event` nodes with temporal properties.
///
/// Each event has:
///   `{id, name, event_date: date('2024-01-DD'), start_time: time('HH:00:00'),
///    created_at: datetime('2024-MM-DDT10:00:00Z')}`
///
/// Events are linked sequentially with `:FOLLOWS` relationships.
pub fn build_temporal_graph(n: usize) -> BenchDb {
    let db = BenchDb::new();
    let batch = RICH_BATCH;
    let mut i = 0usize;
    while i < n {
        let end = (i + batch).min(n);
        db.run(&format!(
            "UNWIND range({i}, {}) AS i \
             CREATE (:Event {{id: i, \
                name: 'event_' + toString(i), \
                event_date: date('2024-01-' + CASE WHEN (i % 28) + 1 < 10 THEN '0' + toString((i % 28) + 1) ELSE toString((i % 28) + 1) END), \
                start_time: time(CASE WHEN i % 24 < 10 THEN '0' + toString(i % 24) ELSE toString(i % 24) END + ':00:00'), \
                created_at: datetime('2024-' + CASE WHEN (i % 12) + 1 < 10 THEN '0' + toString((i % 12) + 1) ELSE toString((i % 12) + 1) END + '-15T10:00:00Z'), \
                priority: i % 5}})",
            end - 1
        ));
        i = end;
    }
    // Link events sequentially
    if n > 1 {
        let mut i = 0usize;
        while i < n - 1 {
            let end = (i + batch).min(n - 1);
            db.run(&format!(
                "UNWIND range({i}, {}) AS i \
                 MATCH (a:Event {{id: i}}), (b:Event {{id: i + 1}}) \
                 CREATE (a)-[:FOLLOWS {{gap_days: (i % 7) + 1}}]->(b)",
                end - 1
            ));
            i = end;
        }
    }
    db
}

// ---------------------------------------------------------------------------
// Spatial graph builder
// ---------------------------------------------------------------------------

/// Build a graph of `:Location` nodes with spatial (point) properties.
///
/// Each location has a Cartesian point `{x, y}` and a geographic point
/// `{latitude, longitude}`.  Locations are connected to their nearest
/// neighbours (by index) with `:CONNECTS_TO` relationships.
pub fn build_spatial_graph(n: usize) -> BenchDb {
    let db = BenchDb::new();
    let batch = RICH_BATCH;
    let mut i = 0usize;
    while i < n {
        let end = (i + batch).min(n);
        // Distribute points in a grid pattern for deterministic distances
        db.run(&format!(
            "UNWIND range({i}, {}) AS i \
             CREATE (:Location {{id: i, \
                name: 'loc_' + toString(i), \
                pos: point({{x: toFloat(i % 100), y: toFloat(i / 100)}}), \
                geo: point({{latitude: 48.0 + toFloat(i % 50) / 10.0, longitude: 2.0 + toFloat(i / 50) / 10.0}}), \
                category: CASE i % 4 WHEN 0 THEN 'restaurant' WHEN 1 THEN 'hotel' WHEN 2 THEN 'museum' ELSE 'park' END \
             }})",
            end - 1
        ));
        i = end;
    }
    // Connect each location to the next 2 locations (ring-like)
    if n > 1 {
        let mut i = 0usize;
        while i < n {
            let end = (i + batch).min(n);
            db.run(&format!(
                "UNWIND range({i}, {}) AS i \
                 MATCH (a:Location {{id: i}}), (b:Location {{id: (i + 1) % {n}}}) \
                 CREATE (a)-[:CONNECTS_TO {{weight: (i % 10) + 1}}]->(b)",
                end - 1
            ));
            i = end;
        }
    }
    db
}

// ---------------------------------------------------------------------------
// Recommendation / e-commerce graph builder
// ---------------------------------------------------------------------------

/// Build a bipartite user-product recommendation graph.
///
/// * `n_users` `:User` nodes with `{id, name, age, tier}`
/// * `n_products` `:Product` nodes with `{id, name, price, category}`
/// * `:BOUGHT` relationships (each user buys 3-8 products)
/// * `:REVIEWED` relationships with `{rating: 1..5}` (subset of purchases)
/// * `:SIMILAR_TO` between products in the same category
pub fn build_recommendation_graph(n_users: usize, n_products: usize) -> BenchDb {
    let db = BenchDb::new();
    let batch = BULK_BATCH;
    let categories = ["Electronics", "Books", "Clothing", "Food", "Sports"];

    // Create users
    let mut i = 0usize;
    while i < n_users {
        let end = (i + batch).min(n_users);
        db.run(&format!(
            "UNWIND range({i}, {}) AS i \
             CREATE (:User {{id: i, \
                name: 'user_' + toString(i), \
                age: 18 + (i % 50), \
                tier: CASE i % 3 WHEN 0 THEN 'gold' WHEN 1 THEN 'silver' ELSE 'bronze' END \
             }})",
            end - 1
        ));
        i = end;
    }

    // Create products
    let mut i = 0usize;
    while i < n_products {
        let end = (i + batch).min(n_products);
        db.run(&format!(
            "UNWIND range({i}, {}) AS i \
             CREATE (:Product {{id: i, \
                name: 'product_' + toString(i), \
                price: 10 + (i % 200), \
                category: CASE i % 5 \
                    WHEN 0 THEN '{c0}' WHEN 1 THEN '{c1}' WHEN 2 THEN '{c2}' \
                    WHEN 3 THEN '{c3}' ELSE '{c4}' END \
             }})",
            end - 1,
            c0 = categories[0],
            c1 = categories[1],
            c2 = categories[2],
            c3 = categories[3],
            c4 = categories[4],
        ));
        i = end;
    }

    // Create BOUGHT relationships (each user buys 3-8 products, deterministic)
    if n_products > 0 {
        let mut i = 0usize;
        while i < n_users {
            let end = (i + batch).min(n_users);
            let n_buys = 5; // fixed for determinism
            for b in 0..n_buys {
                db.run(&format!(
                    "UNWIND range({i}, {}) AS i \
                     MATCH (u:User {{id: i}}), (p:Product {{id: (i * {n_buys} + {b}) % {n_products}}}) \
                     CREATE (u)-[:BOUGHT {{quantity: (i + {b}) % 5 + 1}}]->(p)",
                    end - 1,
                ));
            }
            // Reviews for a subset (first 2 purchases)
            for b in 0..2usize {
                db.run(&format!(
                    "UNWIND range({i}, {}) AS i \
                     MATCH (u:User {{id: i}}), (p:Product {{id: (i * {n_buys} + {b}) % {n_products}}}) \
                     CREATE (u)-[:REVIEWED {{rating: (i + {b}) % 5 + 1}}]->(p)",
                    end - 1,
                ));
            }
            i = end;
        }
    }

    // Create SIMILAR_TO between adjacent products in same category
    if n_products > 5 {
        let mut i = 0usize;
        while i < n_products {
            let end = (i + batch).min(n_products);
            // Connect products that share a category (those 5 apart)
            db.run(&format!(
                "UNWIND range({i}, {}) AS i \
                 MATCH (a:Product {{id: i}}), (b:Product {{id: (i + 5) % {n_products}}}) \
                 WHERE a.category = b.category \
                 CREATE (a)-[:SIMILAR_TO {{score: toFloat((i % 10) + 1) / 10.0}}]->(b)",
                end.min(n_products) - 1,
            ));
            i = end;
        }
    }

    db
}
