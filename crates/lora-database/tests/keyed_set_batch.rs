//! `SET` items that write one node's properties are stored as one record
//! edit when no later item can see the earlier ones (G-76). These pin
//! that the result is the one writing them in turn gives: reads of the
//! node itself, through another variable or a pattern still see each
//! earlier item, constraints still reject what they did, and the WAL
//! replays to the same graph.

mod test_helpers;

use std::path::PathBuf;

use lora_database::{Database, DatabaseOpenOptions, ExecuteOptions, ResultFormat};
use serde_json::json;
use test_helpers::TestDb;

/// The same statements on a graph without constraints (the in-place
/// write path) and with one (the staged path).
fn graphs() -> [TestDb; 2] {
    let plain = TestDb::new();
    let constrained = TestDb::new();
    constrained.run("CREATE CONSTRAINT k FOR (n:N) REQUIRE n.key IS UNIQUE");
    for db in [&plain, &constrained] {
        db.run("CREATE (:N {key: 'a', x: 1}), (:N {key: 'b', x: 2})");
    }
    [plain, constrained]
}

#[test]
fn an_item_that_reads_the_node_sees_the_items_before_it() {
    for db in graphs() {
        db.run("MATCH (n:N {key: 'a'}) SET n.p = 10, n.q = n.p + 1, n.r = n.q * 2");
        assert_eq!(
            db.run("MATCH (n:N {key: 'a'}) RETURN n.p AS p, n.q AS q, n.r AS r"),
            vec![json!({"p": 10, "q": 11, "r": 22})]
        );
        // Through another variable bound to the same node.
        db.run("MATCH (n:N {key: 'b'}), (m:N {key: 'b'}) SET n.p = 5, n.q = m.p");
        assert_eq!(db.scalar("MATCH (n:N {key: 'b'}) RETURN n.q"), json!(5));
        // Through a pattern that finds it again.
        db.run(
            "MATCH (n:N {key: 'b'}) SET n.p = 7, \
             n.q = head([(m:N {key: 'b'}) | m.p])",
        );
        assert_eq!(db.scalar("MATCH (n:N {key: 'b'}) RETURN n.q"), json!(7));
    }
}

#[test]
fn plain_values_are_written_in_order() {
    for db in graphs() {
        db.run(
            "UNWIND [{key: 'a', v: 1, s: 'one'}, {key: 'b', v: 2, s: 'two'}] AS row \
             MATCH (n:N {key: row.key}) \
             SET n.v = row.v, n.s = row.s, n.x = null, n.v = row.v * 100",
        );
        assert_eq!(
            db.run("MATCH (n:N) RETURN n.key AS key, n.v AS v, n.s AS s, n.x AS x ORDER BY key"),
            vec![
                json!({"key": "a", "v": 100, "s": "one", "x": null}),
                json!({"key": "b", "v": 200, "s": "two", "x": null}),
            ]
        );
        db.run("MATCH (n:N {key: 'a'}) SET n += {v: 3, s: null, t: [1, 2]}");
        assert_eq!(
            db.run("MATCH (n:N {key: 'a'}) RETURN n.v AS v, n.s AS s, n.t AS t"),
            vec![json!({"v": 3, "s": null, "t": [1, 2]})]
        );
        // The property indexes follow: the removed value is gone, the new
        // one found.
        db.run("CREATE INDEX nv IF NOT EXISTS FOR (n:N) ON (n.v)");
        db.run("MATCH (n:N {key: 'b'}) SET n.v = 7, n.w = 1");
        assert_eq!(
            db.scalar("MATCH (n:N) WHERE n.v = 7 RETURN n.key"),
            json!("b")
        );
        assert_eq!(
            db.scalar("MATCH (n:N) WHERE n.v = 200 RETURN count(n)"),
            json!(0)
        );
    }
}

#[test]
fn a_constraint_still_rejects_a_batched_write_and_nothing_is_kept() {
    let [_, db] = graphs();
    let err = db.run_err("MATCH (n:N {key: 'b'}) SET n.y = 1, n.key = 'a'");
    assert!(
        err.contains("already exists") || err.contains("22N"),
        "{err}"
    );
    assert_eq!(
        db.run("MATCH (n:N {key: 'b'}) RETURN n.y AS y"),
        vec![json!({"y": null})]
    );
    let err = db.run_err("MATCH (n:N {key: 'b'}) SET n += {y: 1, key: 'a'}");
    assert!(
        err.contains("already exists") || err.contains("22N"),
        "{err}"
    );
    assert_eq!(db.scalar("MATCH (n:N {key: 'b'}) RETURN n.y"), json!(null));
}

#[test]
fn a_constraint_on_two_properties_sees_each_write_in_turn() {
    let db = TestDb::new();
    db.run("CREATE CONSTRAINT pair FOR (n:P) REQUIRE (n.a, n.b) IS UNIQUE");
    db.run("CREATE (:P {id: 1, a: 1, b: 1}), (:P {id: 2, a: 1, b: 3})");
    // In turn the pair goes (5, 3), then (5, 1): neither is held. Checked
    // against the values before the statement, `b = 1` would meet (1, 1),
    // which node 1 holds, and fail.
    db.run("MATCH (n:P {id: 2}) SET n.a = 5, n.b = 1");
    assert_eq!(
        db.run("MATCH (n:P {id: 2}) RETURN n.a AS a, n.b AS b"),
        vec![json!({"a": 5, "b": 1})]
    );
    // Through (1, 1), which node 1 holds: rejected, and nothing is kept.
    let err = db.run_err("MATCH (n:P {id: 2}) SET n.a = 1, n.a = 4");
    assert!(err.contains("22N79"), "{err}");
    assert_eq!(db.scalar("MATCH (n:P {id: 2}) RETURN n.a"), json!(5));
}

struct TmpDir(PathBuf);

impl TmpDir {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "lora-keyed-set-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&path).unwrap();
        Self(path)
    }
}

impl Drop for TmpDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[test]
fn batched_writes_replay_from_the_wal() {
    let dir = TmpDir::new();
    let options = DatabaseOpenOptions::default().with_database_dir(&dir.0);
    let rows = Some(ExecuteOptions {
        format: ResultFormat::Rows,
    });
    let state = |db: &Database<lora_database::InMemoryGraph>| {
        let result = db
            .execute(
                "MATCH (n:N) RETURN n.key AS key, properties(n) AS p ORDER BY key",
                rows,
            )
            .unwrap();
        serde_json::to_value(&result).unwrap()["rows"].clone()
    };
    let before = {
        let db = Database::open_named("app", options.clone()).unwrap();
        db.execute(
            "CREATE CONSTRAINT k FOR (n:N) REQUIRE n.key IS UNIQUE",
            rows,
        )
        .unwrap();
        db.execute(
            "UNWIND range(1, 300) AS i CREATE (:N {key: 'n' + toString(i), dirty: true})",
            rows,
        )
        .unwrap();
        db.execute(
            "UNWIND range(1, 300) AS i MATCH (n:N {key: 'n' + toString(i)}) \
             SET n.dirty = null, n.v = i, n.s = 'v' + toString(i) \
             SET n += {w: i * 2, v: null}",
            rows,
        )
        .unwrap();
        state(&db)
    };
    let db = Database::open_named("app", options).unwrap();
    assert_eq!(state(&db), before);
    assert_eq!(before[0]["p"], json!({"key": "n1", "s": "v1", "w": 2}));
}
