//! Kill-9 probe for a named database's write, `sync()` and open paths.
//!
//! Run with:
//!
//! ```text
//! cargo run --release -p lora-database --example crash_probe -- <dir> <rounds> [preload]
//! ```
//!
//! Each round starts a child that opens `<dir>/probe.loradb`, writes and
//! calls `sync()` in a loop, and prints each write once it is
//! acknowledged; the parent kills it with SIGKILL after a random delay,
//! which lands in writes, in `sync()` and in the open itself. The parent
//! then opens the database and checks that every acknowledged write is
//! there and that the graph is whole: each id at most once, a chain of
//! relationships between consecutive ids, and each node's counter equal
//! to the number of updates acknowledged for it (or one more, for a write
//! killed after it committed and before it was acknowledged).
//!
//! `preload` nodes are written before the first round, so a checkpoint
//! takes long enough to be killed in the middle.
//!
//! The database carries a uniqueness constraint, a range index and a
//! fulltext index, so every open after a kill rebuilds them from the
//! snapshot and the replayed WAL, and the check reads the graph through
//! each of them as well: an index that missed or repeated an entity
//! fails the round like a lost write.

use std::collections::BTreeMap;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use lora_database::{Database, DatabaseOpenOptions, ExecuteOptions, ResultFormat};

fn rows() -> Option<ExecuteOptions> {
    Some(ExecuteOptions {
        format: ResultFormat::Rows,
    })
}

fn options(dir: &Path) -> DatabaseOpenOptions {
    DatabaseOpenOptions {
        segment_target_bytes: 256 * 1024,
        ..DatabaseOpenOptions::default().with_database_dir(dir)
    }
}

fn random_u64() -> u64 {
    let mut bytes = [0u8; 8];
    getrandom::getrandom(&mut bytes).unwrap();
    u64::from_le_bytes(bytes)
}

fn query_rows(db: &Database<lora_database::InMemoryGraph>, query: &str) -> Vec<serde_json::Value> {
    let result = db.execute(query, rows()).unwrap();
    let json = serde_json::to_value(&result).unwrap();
    json["rows"].as_array().cloned().unwrap_or_default()
}

/// Highest id in the database, or -1 when empty.
fn max_id(db: &Database<lora_database::InMemoryGraph>) -> i64 {
    query_rows(db, "MATCH (n:P) RETURN coalesce(max(n.id), -1) AS m")[0]["m"]
        .as_i64()
        .unwrap()
}

/// The constraint and indexes every open rebuilds. Idempotent.
fn ensure_schema(db: &Database<lora_database::InMemoryGraph>) {
    for ddl in [
        "CREATE CONSTRAINT p_id IF NOT EXISTS FOR (n:P) REQUIRE n.id IS UNIQUE",
        "CREATE INDEX p_updates IF NOT EXISTS FOR (n:P) ON (n.updates)",
        "CREATE FULLTEXT INDEX p_text IF NOT EXISTS FOR (n:P|Pad) ON EACH [n.name, n.s]",
    ] {
        db.execute(ddl, rows()).unwrap();
    }
}

/// Child: write forever, printing `c <id>` after a create and `u <id>`
/// after an update, and `s` after a sync.
fn child(dir: &Path) {
    let db = Database::open_named("probe", options(dir)).unwrap();
    ensure_schema(&db);
    println!("open");
    let mut next = max_id(&db) + 1;
    loop {
        let roll = random_u64() % 100;
        if roll < 3 {
            db.sync().unwrap();
            println!("s");
        } else if roll < 30 && next > 0 {
            let id = (random_u64() % next as u64) as i64;
            db.execute(
                &format!("MATCH (n:P {{id: {id}}}) SET n.updates = n.updates + 1"),
                rows(),
            )
            .unwrap();
            println!("u {id}");
        } else {
            let id = next;
            db.execute(
                &format!(
                    "CREATE (n:P {{id: {id}, updates: 0, name: 'p' + toString({id}), tags: ['a', {id}]}}) \
                     WITH n OPTIONAL MATCH (prev:P {{id: {id} - 1}}) \
                     FOREACH (_ IN CASE WHEN prev IS NULL THEN [] ELSE [1] END | CREATE (prev)-[:NEXT]->(n))"
                ),
                rows(),
            )
            .unwrap();
            next += 1;
            println!("c {id}");
        }
    }
}

struct Acked {
    max_created: i64,
    updates: BTreeMap<i64, i64>,
    syncs: u64,
}

fn verify(dir: &Path, acked: &Acked) {
    let db = Database::open_named("probe", options(dir)).unwrap();
    let max = max_id(&db);
    assert!(
        max >= acked.max_created && max <= acked.max_created + 1,
        "max id {max}, acknowledged {}",
        acked.max_created
    );
    let counts = &query_rows(
        &db,
        "MATCH (n:P) RETURN count(n) AS n, count(DISTINCT n.id) AS d",
    )[0];
    assert_eq!(counts["n"].as_i64().unwrap(), max + 1, "node count");
    assert_eq!(counts["d"].as_i64().unwrap(), max + 1, "distinct ids");
    let chain = &query_rows(
        &db,
        "MATCH (a:P)-[:NEXT]->(b:P) RETURN count(*) AS n, sum(CASE WHEN b.id = a.id + 1 THEN 1 ELSE 0 END) AS ok",
    )[0];
    assert_eq!(chain["n"].as_i64().unwrap(), max.max(0), "chain length");
    assert_eq!(chain["ok"].as_i64().unwrap(), max.max(0), "chain order");
    let stored: BTreeMap<i64, i64> = query_rows(
        &db,
        "MATCH (n:P) WHERE n.updates > 0 RETURN n.id AS id, n.updates AS u",
    )
    .into_iter()
    .map(|row| (row["id"].as_i64().unwrap(), row["u"].as_i64().unwrap()))
    .collect();
    let mut extra = 0;
    for (id, want) in &acked.updates {
        let got = stored.get(id).copied().unwrap_or(0);
        assert!(
            got >= *want,
            "node {id}: {got} updates, {want} acknowledged"
        );
        extra += got - want;
    }
    for (id, got) in &stored {
        if !acked.updates.contains_key(id) {
            extra += got;
        }
    }
    assert!(extra <= 1, "{extra} unacknowledged updates");

    // The same graph through the indexes the open rebuilt.
    let ranged = query_rows(&db, "MATCH (n:P) WHERE n.updates >= 0 RETURN count(n) AS n");
    assert_eq!(ranged[0]["n"].as_i64().unwrap(), max + 1, "range index");
    if max >= 0 {
        for id in [0, max / 2, max] {
            let seek = query_rows(
                &db,
                &format!("MATCH (n:P {{id: {id}}}) RETURN count(n) AS n"),
            );
            assert_eq!(seek[0]["n"].as_i64().unwrap(), 1, "seek of id {id}");
        }
        // Every node's name is the one token `p<id>`: the ten digit
        // prefixes together find each node once.
        let mut found = 0;
        for digit in 0..10 {
            let hits = query_rows(
                &db,
                &format!(
                    "CALL db.index.fulltext.queryNodes('p_text', 'p{digit}*') YIELD node \
                     RETURN count(node) AS n, count(DISTINCT node.id) AS d"
                ),
            );
            assert_eq!(hits[0]["n"], hits[0]["d"], "fulltext hits repeat a node");
            found += hits[0]["n"].as_i64().unwrap();
        }
        assert_eq!(found, max + 1, "fulltext index");
    }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.get(1).map(String::as_str) == Some("child") {
        child(Path::new(&args[2]));
        return;
    }
    let dir = PathBuf::from(&args[1]);
    let rounds: usize = args[2].parse().unwrap();
    let preload: usize = args.get(3).map_or(0, |n| n.parse().unwrap());
    std::fs::create_dir_all(&dir).unwrap();

    ensure_schema(&Database::open_named("probe", options(&dir)).unwrap());
    if preload > 0 {
        let db = Database::open_named("probe", options(&dir)).unwrap();
        db.execute(
            &format!(
                "UNWIND list.range(0, {}) AS i CREATE (:Pad {{i: i, s: 'pad' + toString(i)}})",
                preload - 1
            ),
            rows(),
        )
        .unwrap();
        db.sync().unwrap();
    }

    let mut acked = {
        let db = Database::open_named("probe", options(&dir)).unwrap();
        let updates = query_rows(
            &db,
            "MATCH (n:P) WHERE n.updates > 0 RETURN n.id AS id, n.updates AS u",
        )
        .into_iter()
        .map(|row| (row["id"].as_i64().unwrap(), row["u"].as_i64().unwrap()))
        .collect();
        Acked {
            max_created: max_id(&db),
            updates,
            syncs: 0,
        }
    };

    let exe = std::env::current_exe().unwrap();
    let started = Instant::now();
    let mut killed_in_open = 0;
    for round in 0..rounds {
        let mut child = Command::new(&exe)
            .arg("child")
            .arg(&dir)
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        let stdout = child.stdout.take().unwrap();
        let reader = std::thread::spawn(move || {
            BufReader::new(stdout)
                .lines()
                .map_while(Result::ok)
                .collect::<Vec<_>>()
        });
        std::thread::sleep(Duration::from_millis(50 + random_u64() % 1500));
        child.kill().unwrap();
        child.wait().unwrap();
        let lines = reader.join().unwrap();
        if !lines.iter().any(|l| l == "open") {
            killed_in_open += 1;
        }
        for line in &lines {
            match line.split_once(' ') {
                Some(("c", id)) => acked.max_created = id.parse().unwrap(),
                Some(("u", id)) => *acked.updates.entry(id.parse().unwrap()).or_insert(0) += 1,
                _ if line == "s" => acked.syncs += 1,
                _ => {}
            }
        }
        verify(&dir, &acked);
        // The verify's own open counts as one more clean open: an
        // unacknowledged write it found is now part of the history.
        let db = Database::open_named("probe", options(&dir)).unwrap();
        acked.max_created = max_id(&db);
        acked.updates = query_rows(
            &db,
            "MATCH (n:P) WHERE n.updates > 0 RETURN n.id AS id, n.updates AS u",
        )
        .into_iter()
        .map(|row| (row["id"].as_i64().unwrap(), row["u"].as_i64().unwrap()))
        .collect();
        drop(db);
        if (round + 1) % 10 == 0 {
            println!(
                "round {:>4}: {} nodes, {} syncs acknowledged, {} kills during open, {:.0} s",
                round + 1,
                acked.max_created + 1,
                acked.syncs,
                killed_in_open,
                started.elapsed().as_secs_f64()
            );
        }
    }
    println!("ok: {rounds} rounds");
}
