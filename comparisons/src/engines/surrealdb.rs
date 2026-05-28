use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use surrealdb::{
    engine::local::{Db, Mem, SurrealKv},
    Surreal,
};
use tempfile::TempDir;
use tokio::runtime::{Builder, Runtime};

use crate::engine::DbHandle;
use crate::fixtures::{Fixture, FixtureKind};

static SURREAL_DB_ID: AtomicUsize = AtomicUsize::new(0);

fn build_runtime() -> Runtime {
    Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("failed to build surrealdb tokio runtime")
}

fn use_namespace(rt: &Runtime, db: &Surreal<Db>) {
    rt.block_on(async {
        let id = SURREAL_DB_ID.fetch_add(1, Ordering::Relaxed);
        db.use_ns("comparison")
            .use_db(format!("bench_{id}"))
            .await
            .expect("surrealdb namespace/database selection failed");
    });
}

pub fn fresh() -> DbHandle {
    let rt = Arc::new(build_runtime());
    let db = rt.block_on(async {
        Surreal::new::<Mem>(())
            .await
            .expect("surrealdb in-memory open failed")
    });
    use_namespace(&rt, &db);
    DbHandle::Surreal { rt, db, dir: None }
}

pub fn fresh_persistent() -> DbHandle {
    let rt = Arc::new(build_runtime());
    let dir = TempDir::new().expect("surrealdb_kv: tempdir create failed");
    let path = dir.path().to_path_buf();
    let db = rt.block_on(async {
        Surreal::new::<SurrealKv>(path)
            .await
            .expect("surrealdb SurrealKv open failed")
    });
    use_namespace(&rt, &db);
    DbHandle::Surreal {
        rt,
        db,
        dir: Some(dir),
    }
}

pub fn seed(handle: &mut DbHandle, fixture: Fixture) {
    let DbHandle::Surreal { rt, db, .. } = handle else {
        panic!("surrealdb seed: wrong handle variant");
    };
    match fixture.kind {
        FixtureKind::Empty => {}
        FixtureKind::Nodes => seed_nodes(rt, db, fixture.size),
        FixtureKind::Chain => seed_chain(rt, db, fixture.size),
        FixtureKind::Star => seed_star(rt, db, fixture.size),
        FixtureKind::Social => seed_social(rt, db, fixture.size),
    }
}

/// SurrealDB does not auto-index. Build the index against the most
/// likely target table for the given prop. This is a heuristic — the
/// `node` table is the only one we currently bench property indexes on.
pub fn create_index(handle: &DbHandle, prop: &str) {
    let DbHandle::Surreal { rt, db, .. } = handle else {
        panic!("surrealdb create_index: wrong handle variant");
    };
    let sql = format!("DEFINE INDEX OVERWRITE {prop}_idx ON node FIELDS {prop};");
    rt.block_on(async { db.query(&sql).await })
        .unwrap_or_else(|e| panic!("surrealdb create_index failed: {sql}\nerror: {e}"));
}

pub fn execute(handle: &DbHandle, q: &str) {
    let DbHandle::Surreal { rt, db, .. } = handle else {
        panic!("surrealdb execute: wrong handle variant");
    };
    rt.block_on(async { db.query(q).await })
        .unwrap_or_else(|e| panic!("surrealdb query failed: {q}\nerror: {e}"));
}

fn run(rt: &Runtime, db: &Surreal<Db>, sql: &str) {
    rt.block_on(async { db.query(sql).await })
        .unwrap_or_else(|e| panic!("surrealdb seed sql failed: {sql}\nerror: {e}"));
}

fn seed_nodes(rt: &Runtime, db: &Surreal<Db>, n: usize) {
    let mut sql = String::new();
    for i in 0..n {
        sql.push_str(&format!(
            "CREATE node:{i} SET id = {i}, name = 'node_{i}', value = {} RETURN NONE;\n",
            i % 100
        ));
    }
    run(rt, db, &sql);
}

fn seed_chain(rt: &Runtime, db: &Surreal<Db>, len: usize) {
    let mut sql = String::new();
    sql.push_str("DEFINE TABLE next TYPE RELATION IN chain OUT chain;\n");
    for i in 0..len {
        sql.push_str(&format!(
            "CREATE chain:{i} SET idx = {i}, name = 'chain_{i}' RETURN NONE;\n"
        ));
    }
    for i in 0..len.saturating_sub(1) {
        sql.push_str(&format!(
            "RELATE chain:{i}->next:{i}->chain:{} SET step = {i} RETURN NONE;\n",
            i + 1
        ));
    }
    run(rt, db, &sql);
}

fn seed_star(rt: &Runtime, db: &Surreal<Db>, spokes: usize) {
    let mut sql = String::new();
    sql.push_str("DEFINE TABLE arm TYPE RELATION IN hub OUT leaf;\n");
    sql.push_str("CREATE hub:center SET name = 'center' RETURN NONE;\n");
    for i in 0..spokes {
        sql.push_str(&format!("CREATE leaf:{i} SET idx = {i} RETURN NONE;\n"));
        sql.push_str(&format!(
            "RELATE hub:center->arm:{i}->leaf:{i} RETURN NONE;\n"
        ));
    }
    run(rt, db, &sql);
}

fn seed_social(rt: &Runtime, db: &Surreal<Db>, n: usize) {
    let mut sql = String::new();
    sql.push_str("DEFINE TABLE knows TYPE RELATION IN person OUT person;\n");
    for i in 0..n {
        sql.push_str(&format!(
            "CREATE person:{i} SET idx = {i}, name = 'person_{i}' RETURN NONE;\n"
        ));
    }
    for i in 0..n {
        for offset in 1..=2usize.min(n.saturating_sub(1)) {
            let other = (i + offset) % n;
            let edge = (i * 2) + (offset - 1);
            let strength = (i + offset) % 5;
            sql.push_str(&format!(
                "RELATE person:{i}->knows:{edge}->person:{other} \
                 SET strength = {strength} RETURN NONE;\n"
            ));
        }
    }
    run(rt, db, &sql);
}
