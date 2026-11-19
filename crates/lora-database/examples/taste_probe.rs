//! Taste (Festimap) query shapes, timed: `cargo run --release --example
//! taste_probe [section...]`. Sections: `in`, `composite`, `notnull`, `vector`, and
//! `hnsw=<count>[:<clusters>[:<noise>]]` / `hnsw-ingest=<count>x<batch>`, `vecmem` (not run by default). Each prints the
//! median of repeated runs, so a build before and after a change can be
//! compared line by line.
use lora_database::{Database, ExecuteOptions, LoraValue, ResultFormat};
use std::collections::BTreeMap;
use std::time::Instant;

type Params = BTreeMap<String, LoraValue>;
type Db = Database<lora_store::InMemoryGraph>;

fn median_ms(db: &Db, q: &str, params: &Params, runs: usize) -> f64 {
    let o = Some(ExecuteOptions {
        format: ResultFormat::Rows,
    });
    db.execute_with_params(q, o, params.clone()).unwrap();
    let mut samples: Vec<f64> = (0..runs)
        .map(|_| {
            let t = Instant::now();
            db.execute_with_params(q, o, params.clone()).unwrap();
            t.elapsed().as_secs_f64() * 1000.0
        })
        .collect();
    samples.sort_by(|a, b| a.total_cmp(b));
    samples[runs / 2]
}

/// N-10 / N-10b: 30,000 genre checks against a 1000-string list, held
/// directly, inside a map, and inside a list of lists.
fn in_section() {
    let db = Database::in_memory();
    let none = Params::new();
    let base = "WITH [x IN range(1, 1000) | 'g' + toString(x)] AS l \
                WITH l, {l: l} AS m, [l, l] AS tiers \
                UNWIND range(1, 30000) AS i WITH l, m, tiers, i % 2 AS r, 'g' + toString(i % 2000) AS g";
    for (name, probe) in [
        ("no IN (overhead)", "g <> 'x'"),
        ("x IN l", "g IN l"),
        ("x IN m.l", "g IN m.l"),
        ("x IN tiers[1]", "g IN tiers[1]"),
        // N-10c: the list picked per row.
        ("x IN tiers[r]", "g IN tiers[r]"),
        ("x IN tiers[r+0]", "g IN tiers[r + 0]"),
        (
            "any(r IN [0,1] WHERE x IN tiers[r])",
            "any(k IN [0, 1] WHERE g IN tiers[k])",
        ),
    ] {
        let q = format!("{base} WHERE {probe} RETURN count(*) AS n");
        println!(
            "in  {name:<36} 30k checks  p50_ms={:.2}",
            median_ms(&db, &q, &none, 9)
        );
    }
    let list: Vec<LoraValue> = (1..=1000)
        .map(|x| LoraValue::String(format!("g{x}")))
        .collect();
    let params = Params::from([("l".to_string(), LoraValue::List(list))]);
    let q = "UNWIND range(1, 30000) AS i WITH 'g' + toString(i % 2000) AS g \
             WHERE g IN $l RETURN count(*) AS n";
    println!(
        "in  {:<16} 30k checks  p50_ms={:.2}",
        "x IN $l",
        median_ms(&db, q, &params, 9)
    );
}

/// N-01: the 20 nearest events (64-dim) within a box and a date window,
/// over `n` events.
fn vector_section(n: usize) {
    let db = Database::in_memory();
    db.execute(
        "CREATE VECTOR INDEX ev FOR (e:Event) ON (e.taste) OPTIONS {indexConfig: \
         {`vector.dimensions`: 64, `vector.similarity_function`: 'cosine'}}",
        None,
    )
    .unwrap();
    db.execute("CREATE INDEX ev_at FOR (e:Event) ON (e.at)", None)
        .unwrap();
    db.execute("CREATE POINT INDEX ev_loc FOR (e:Event) ON (e.loc)", None)
        .unwrap();
    let t = Instant::now();
    db.execute(
        &format!(
            "UNWIND range(1, {n}) AS i \
             CREATE (:Event {{k: i, at: i % 365, \
               loc: point({{longitude: -10.0 + (i * 7919 % 4000) * 0.01, latitude: 36.0 + (i * 104729 % 3000) * 0.01}}), \
               taste: [d IN range(1, 64) | toFloat((i * d) % 17) - 8.0]::VECTOR<FLOAT32>(64)}})"
        ),
        None,
    )
    .unwrap();
    let load = t.elapsed().as_secs_f64();
    // About the Netherlands within Europe, and a 60-day window.
    let point = |lon: f64, lat: f64| {
        LoraValue::Point(Box::new(lora_store::LoraPoint {
            x: lon,
            y: lat,
            z: None,
            srid: 4326,
        }))
    };
    let query: Vec<LoraValue> = (1..=64)
        .map(|d| LoraValue::Float(((d * 5) % 13) as f64 - 6.0))
        .collect();
    let params = Params::from([
        ("q".to_string(), LoraValue::List(query)),
        ("sw".to_string(), point(3.3, 50.7)),
        ("ne".to_string(), point(7.2, 53.6)),
        ("from".to_string(), LoraValue::Int(100)),
        ("to".to_string(), LoraValue::Int(160)),
    ]);
    let in_box = median_ms(
        &db,
        "MATCH (e:Event) WHERE geo.within_bbox(e.loc, $sw, $ne) AND e.at >= $from AND e.at < $to \
         RETURN count(*) AS n",
        &params,
        5,
    );
    println!("vec n={n} load_s={load:.1} (box+window read alone p50_ms={in_box:.2})");
    let cases = [
        (
            "exact scan",
            "MATCH (e:Event) WHERE geo.within_bbox(e.loc, $sw, $ne) AND e.at >= $from AND e.at < $to \
             RETURN e.k AS k ORDER BY vector.similarity(e.taste, $q) DESC LIMIT 20",
        ),
        (
            "ids+restrictTo",
            "MATCH (e:Event) WHERE geo.within_bbox(e.loc, $sw, $ne) AND e.at >= $from AND e.at < $to \
             WITH collect(id(e)) AS ids \
             CALL db.index.vector.queryNodes('ev', 20, $q, {restrictTo: ids}) YIELD node, score \
             RETURN node.k AS k",
        ),
        (
            "where option",
            "CALL db.index.vector.queryNodes('ev', 20, $q, {where: \
               {loc: {withinBBox: [$sw, $ne]}, at: {gte: $from, lt: $to}}}) YIELD node, score \
             RETURN node.k AS k",
        ),
    ];
    for (name, q) in cases {
        match db.execute_with_params(q, None, params.clone()) {
            Ok(_) => println!(
                "vec n={n} {name:<15} p50_ms={:.2}",
                median_ms(&db, q, &params, 9)
            ),
            Err(e) => println!(
                "vec n={n} {name:<15} unsupported ({})",
                e.to_string().lines().next().unwrap_or("")
            ),
        }
    }
}

/// N-08: `IS NOT NULL` on a property 200 of 200,000 nodes hold (the
/// legacy `tasteRank` cleanup), and on one every node holds (the worst
/// case for the index seek, which then lists the whole label).
fn not_null_section() {
    let db = Database::in_memory();
    let none = Params::new();
    db.execute("CREATE INDEX e_rank FOR (e:Event) ON (e.tasteRank)", None)
        .unwrap();
    db.execute("CREATE INDEX e_at FOR (e:Event) ON (e.at)", None)
        .unwrap();
    db.execute(
        "UNWIND range(1, 200000) AS i CREATE (:Event {k: i, at: i % 365, \
           tasteRank: CASE WHEN i % 1000 = 0 THEN toFloat(i) / 200000 END})",
        None,
    )
    .unwrap();
    for (name, q) in [
        (
            "sparse 200/200k",
            "MATCH (e:Event) WHERE e.tasteRank IS NOT NULL RETURN count(*) AS n",
        ),
        (
            "dense 200k/200k",
            "MATCH (e:Event) WHERE e.at IS NOT NULL RETURN count(*) AS n",
        ),
    ] {
        println!("nn  {name:<16} p50_ms={:.2}", median_ms(&db, q, &none, 9));
    }
}

/// N-06: a person's latest events, `person = $p AND at >= $from ORDER BY
/// at DESC LIMIT 20`, over 300,000 events of 20 people (15,000 each),
/// with a RANGE index on each property, and with a composite index on
/// `(person, at)` added (the before build has no composite seek, so its
/// second line shows that the index alone changes nothing).
fn composite_section() {
    let db = Database::in_memory();
    db.execute("CREATE INDEX e_person FOR (e:Event) ON (e.person)", None)
        .unwrap();
    db.execute("CREATE INDEX e_at FOR (e:Event) ON (e.at)", None)
        .unwrap();
    db.execute(
        "UNWIND range(1, 300000) AS i CREATE (:Event {k: i, person: 'p' + toString(i % 20), at: i})",
        None,
    )
    .unwrap();
    let params = Params::from([
        ("p".to_string(), LoraValue::String("p7".into())),
        ("from".to_string(), LoraValue::Int(1000)),
    ]);
    let q = "MATCH (e:Event) WHERE e.person = $p AND e.at >= $from \
             RETURN e.k AS k ORDER BY e.at DESC LIMIT 20";
    println!(
        "cmp single-property indexes  p50_ms={:.2}",
        median_ms(&db, q, &params, 9)
    );
    db.execute(
        "CREATE INDEX e_person_at FOR (e:Event) ON (e.person, e.at)",
        None,
    )
    .unwrap();
    println!(
        "cmp + (person, at) index     p50_ms={:.2}",
        median_ms(&db, q, &params, 9)
    );
}

/// A deterministic pseudo-random number in [0, 1) from two Cypher
/// expressions (the shader hash), for embedding-like test vectors.
fn hash(a: &str, b: &str) -> String {
    format!("(sin(toFloat({a}) * 12.9898 + toFloat({b}) * 78.233) * 43758.5453 - floor(sin(toFloat({a}) * 12.9898 + toFloat({b}) * 78.233) * 43758.5453))")
}

/// The HNSW extra: building a 64-dim cosine HNSW index over `n` existing
/// events, embedding-like (`clusters` centres, each coordinate moved by up
/// to `noise`) (taste: 100,000 did not finish in 10 minutes), then the p50
/// of a top-20 query and the mean recall against the exact top 20 over 40
/// queries, each an event's own vector.
fn hnsw_section(n: usize, clusters: usize, noise: f64, extra: &str) {
    let db = Database::in_memory();
    db.execute(
        &format!(
            "UNWIND range(1, {n}) AS i \
             WITH i, [d IN range(1, 64) | 2 * {HASH} - 1] AS centre, [d IN range(1, 64) | 2 * {NOISE} - 1] AS noise \
             CREATE (:Event {{k: i, taste: [d IN range(0, 63) | centre[d] + {noise} * noise[d]]::VECTOR<FLOAT32>(64)}})",
            HASH = hash(&format!("i % {clusters}"), "d"),
            NOISE = hash("i + 1000", "d"),
        ),
        None,
    )
    .unwrap();
    db.execute("CREATE INDEX ev_k FOR (e:Event) ON (e.k)", None)
        .unwrap();
    let t = Instant::now();
    db.execute(
        &format!(
            "CREATE VECTOR INDEX ev FOR (e:Event) ON (e.taste) OPTIONS {{indexConfig: \
             {{`vector.dimensions`: 64, `vector.similarity_function`: 'cosine', `vector.indexProvider`: 'hnsw'{extra}}}}}"
        ),
        None,
    )
    .unwrap();
    let build = t.elapsed().as_secs_f64();
    // HNSW_QUERY=mix: each query the mean of two events from different
    // clusters, as a taste profile sits between the genres it likes.
    let mix = std::env::var("HNSW_QUERY").is_ok_and(|q| q == "mix");
    let q = if mix {
        "MATCH (a:Event {k: $k}), (b:Event {k: $k2}) \
         WITH vector([d IN range(0, 63) | (vector.coordinates(a.taste)[d] + vector.coordinates(b.taste)[d]) / 2], 64, FLOAT32) AS qv"
    } else {
        "MATCH (a:Event {k: $k}) WITH a.taste AS qv"
    };
    let ann = format!(
        "{q} CALL db.index.vector.queryNodes('ev', 20, qv) YIELD node RETURN collect(node.k) AS ks"
    );
    let exact = format!(
        "{q} MATCH (e:Event) WITH e ORDER BY vector.similarity(e.taste, qv, 'cosine') DESC, e.k LIMIT 20 \
         RETURN collect(e.k) AS ks"
    );
    let (ann, exact) = (ann.as_str(), exact.as_str());
    fn find_ks(v: &serde_json::Value) -> Option<Vec<i64>> {
        match v {
            serde_json::Value::Object(m) => match m.get("ks") {
                Some(serde_json::Value::Array(ks)) => {
                    Some(ks.iter().filter_map(|k| k.as_i64()).collect())
                }
                _ => m.values().find_map(find_ks),
            },
            serde_json::Value::Array(items) => items.iter().find_map(find_ks),
            _ => None,
        }
    }
    let ks = |q: &str, params: &Params| -> Vec<i64> {
        let rows = Some(ExecuteOptions {
            format: ResultFormat::Rows,
        });
        let r = db.execute_with_params(q, rows, params.clone()).unwrap();
        find_ks(&serde_json::to_value(&r).unwrap()).unwrap_or_default()
    };
    let queries: Vec<Params> = (0..40)
        .map(|j| {
            Params::from([
                ("k".to_string(), LoraValue::Int(1 + (j * 7919) % n as i64)),
                (
                    "k2".to_string(),
                    LoraValue::Int(1 + (j * 104_729 + 17) % n as i64),
                ),
            ])
        })
        .collect();
    let mut recall = 0.0;
    for params in &queries {
        let (a, e) = (ks(ann, params), ks(exact, params));
        recall += a.iter().filter(|k| e.contains(k)).count() as f64 / e.len().max(1) as f64;
    }
    recall /= queries.len() as f64;
    println!(
        "hnsw n={n} clusters={clusters} noise={noise} mix={mix} build_s={build:.2} query_p50_ms={:.2} recall@20={recall:.3}",
        median_ms(&db, ann, &queries[0], 9)
    );
}

/// HNSW maintenance on ingest: the index exists and `n` events arrive in
/// transactions of `batch` rows, as an ingest job writes them.
fn hnsw_ingest_section(n: usize, batch: usize) {
    let db = Database::in_memory();
    db.execute(
        "CREATE VECTOR INDEX ev FOR (e:Event) ON (e.taste) OPTIONS {indexConfig: \
         {`vector.dimensions`: 64, `vector.similarity_function`: 'cosine', `vector.indexProvider`: 'hnsw'}}",
        None,
    )
    .unwrap();
    let t = Instant::now();
    for start in (1..=n).step_by(batch) {
        let end = (start + batch - 1).min(n);
        db.execute(
            &format!(
                "UNWIND range({start}, {end}) AS i \
                 WITH i, [d IN range(1, 64) | 2 * {HASH} - 1] AS centre, [d IN range(1, 64) | 2 * {NOISE} - 1] AS noise \
                 CREATE (:Event {{k: i, taste: [d IN range(0, 63) | centre[d] + 0.25 * noise[d]]::VECTOR<FLOAT32>(64)}})",
                HASH = hash("i % 40", "d"),
                NOISE = hash("i + 1000", "d"),
            ),
            None,
        )
        .unwrap();
    }
    println!(
        "hnsw-ingest n={n} batch={batch} total_s={:.2}",
        t.elapsed().as_secs_f64()
    );
}

/// The vector-memory extra: bytes per node for a 64-dim FLOAT32 vector
/// (256 raw) in the record store, and in a flat and an HNSW index, from the
/// graph's memory estimate over 20,000 nodes.
fn vector_memory_section() {
    const N: usize = 20_000;
    let per_node = |bytes: usize| bytes as f64 / N as f64;
    let with = |index: Option<&str>| {
        let db = Database::in_memory();
        if let Some(provider) = index {
            db.execute(
                &format!(
                    "CREATE VECTOR INDEX ev FOR (e:Event) ON (e.taste) OPTIONS {{indexConfig: \
                     {{`vector.dimensions`: 64, `vector.similarity_function`: 'cosine', `vector.indexProvider`: '{provider}'}}}}"
                ),
                None,
            )
            .unwrap();
        }
        db.execute(
            &format!(
                "UNWIND range(1, {N}) AS i \
                 CREATE (:Event {{taste: [d IN range(1, 64) | 2 * {HASH} - 1]::VECTOR<FLOAT32>(64)}})",
                HASH = hash("i", "d"),
            ),
            None,
        )
        .unwrap();
        db.with_store(|g| g.memory_estimate())
    };
    let bare = with(None);
    let plain = Database::in_memory();
    plain
        .execute(&format!("UNWIND range(1, {N}) AS i CREATE (:Event)"), None)
        .unwrap();
    let empty = plain.with_store(|g| g.memory_estimate());
    println!(
        "mem record store: {:.0} B/node for the vector (node without it: {:.0})",
        per_node(bare.nodes_bytes - empty.nodes_bytes),
        per_node(empty.nodes_bytes)
    );
    for provider in ["flat", "hnsw"] {
        let r = with(Some(provider));
        println!(
            "mem {provider} index: {:.0} B/node",
            per_node(r.vector_index_bytes)
        );
    }
}

/// N-07: where an Activity's bytes go. The shape of Festimap's bench:
/// 13 properties, mostly short strings that repeat, a DID and an ABOUT edge.
fn activity_memory_section(typed: bool) {
    const N: usize = 200_000;
    let db = Database::in_memory();
    db.execute("CREATE (:Person {key: 'p'}), (:Event {key: 'e'})", None)
        .unwrap();
    let empty = db.with_store(|g| g.memory_estimate());
    for from in (0..N).step_by(20_000) {
        db.execute(
            &format!(
                "MATCH (p:Person {{key: 'p'}}), (e:Event {{key: 'e'}}) \
                 UNWIND range({from}, {to}) AS i \
                 CREATE (p)-[:DID]->(a:Activity {{id: 'a-' + toString(i) + '-0123456789abcdef0123456789', \
                   v: 1, action: 'view', kind: 'event', target: 'event-' + toString(i % 5000), \
                   name: 'Event ' + toString(i % 5000), at: {AT}, \
                   receivedAt: {RECEIVED}, session: 's-0123456789', surface: 'discover', \
                   platform: 'ios', build: '1.0.0', seconds: 12}}) \
                 CREATE (a)-[:ABOUT]->(e)",
                to = from + 19_999,
                AT = if typed {
                    "datetime('2026-11-18T12:00:00Z') + duration({milliseconds: i % 1000})"
                } else {
                    "'2026-11-18T12:00:00.' + toString(i % 1000) + 'Z'"
                },
                RECEIVED = if typed {
                    "datetime('2026-11-18T12:00:01.000Z')"
                } else {
                    "'2026-11-18T12:00:01.000Z'"
                },
            ),
            None,
        )
        .unwrap();
    }
    let r = db.with_store(|g| g.memory_estimate());
    let per = |a: usize, b: usize| (a - b) as f64 / N as f64;
    println!(
        "activity n={N} typed_times={typed} B/activity: nodes={:.0} rels={:.0} out={:.0} in={:.0} labels+types={:.0} indexes={:.0} stats={:.0} total={:.0}",
        per(r.nodes_bytes, empty.nodes_bytes),
        per(r.relationships_bytes, empty.relationships_bytes),
        per(r.outgoing_bytes, empty.outgoing_bytes),
        per(r.incoming_bytes, empty.incoming_bytes),
        per(r.label_index_bytes + r.type_index_bytes, empty.label_index_bytes + empty.type_index_bytes),
        per(
            r.property_index_bytes + r.sorted_index_bytes,
            empty.property_index_bytes + empty.sorted_index_bytes
        ),
        per(r.distinct_stats_bytes, empty.distinct_stats_bytes),
        per(r.total_bytes(), empty.total_bytes()),
    );
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let on = |s: &str| args.is_empty() || args.iter().any(|a| a == s);
    if on("in") {
        in_section();
    }
    if let Some(n) = args.iter().find_map(|a| a.strip_prefix("hnsw=")) {
        // hnsw=<count>[:<clusters>[:<noise>]]; extra index options from HNSW_OPTS.
        let mut parts = n.split(':');
        let count = parts.next().unwrap().parse().expect("hnsw=<count>");
        let clusters = parts.next().map_or(40, |c| c.parse().expect("clusters"));
        let noise = parts.next().map_or(0.25, |x| x.parse().expect("noise"));
        let extra = std::env::var("HNSW_OPTS").unwrap_or_default();
        hnsw_section(count, clusters, noise, &extra);
    }
    if let Some(spec) = args.iter().find_map(|a| a.strip_prefix("hnsw-ingest=")) {
        let (n, batch) = spec.split_once('x').expect("hnsw-ingest=<count>x<batch>");
        hnsw_ingest_section(n.parse().unwrap(), batch.parse().unwrap());
    }
    if args.iter().any(|a| a == "activity") {
        activity_memory_section(false);
        activity_memory_section(true);
    }
    if args.iter().any(|a| a == "vecmem") {
        vector_memory_section();
    }
    if on("composite") {
        composite_section();
    }
    if on("notnull") {
        not_null_section();
    }
    if on("vector") {
        for n in [30_000, 300_000] {
            vector_section(n);
        }
    }
}
