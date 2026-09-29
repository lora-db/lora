//! Bulk-load cost with and without schema declared up front.
use lora_database::Database;
use std::time::Instant;

fn load(n: usize, ddl: &[&str]) -> f64 {
    let db = Database::in_memory();
    for q in ddl {
        db.execute(q, None).unwrap();
    }
    let t = Instant::now();
    db.execute(
        &format!("UNWIND range(0, {}) AS i CREATE (:Festival {{key: 'f' + toString(i), name: 'n' + toString(i)}})", n - 1),
        None,
    )
    .unwrap();
    t.elapsed().as_secs_f64() * 1000.0
}

fn main() {
    let unique = "CREATE CONSTRAINT k FOR (n:Festival) REQUIRE n.key IS UNIQUE";
    let range = "CREATE INDEX r FOR (n:Festival) ON (n.name)";
    let text = "CREATE FULLTEXT INDEX ft FOR (n:Festival) ON EACH [n.name]";
    for n in [10_000, 20_000, 40_000] {
        println!(
            "bulk n={n} none_ms={:.1} unique_ms={:.1} range_ms={:.1} fulltext_ms={:.1}",
            load(n, &[]),
            load(n, &[unique]),
            load(n, &[range]),
            load(n, &[text]),
        );
    }
}
