//! Festimap P1-4: keyset page latency vs label size.
use lora_database::{Database, ExecuteOptions, LoraValue, ResultFormat};
use std::collections::BTreeMap;
use std::time::Instant;

fn main() {
    let q = "MATCH (f:Festival) WHERE f.key > $after RETURN f.key AS key ORDER BY f.key LIMIT 20";
    for n in [20_000usize, 1_000_000] {
        let db = Database::in_memory();
        db.execute(
            "CREATE CONSTRAINT k IF NOT EXISTS FOR (n:Festival) REQUIRE n.key IS UNIQUE",
            None,
        )
        .unwrap();
        let t = Instant::now();
        db.execute(&format!("UNWIND range(0, {}) AS i CREATE (:Festival {{key: 'f' + substring('0000000' + toString(i), size(toString(i)))}})", n - 1), None).unwrap();
        let load = t.elapsed().as_secs_f64();
        let params = BTreeMap::from([(
            "after".to_string(),
            LoraValue::String(format!("f{:07}", n / 2)),
        )]);
        let o = Some(ExecuteOptions {
            format: ResultFormat::Rows,
        });
        for _ in 0..20 {
            db.execute_with_params(q, o, params.clone()).unwrap();
        }
        let mut samples: Vec<f64> = (0..200)
            .map(|_| {
                let t = Instant::now();
                db.execute_with_params(q, o, params.clone()).unwrap();
                t.elapsed().as_secs_f64() * 1000.0
            })
            .collect();
        samples.sort_by(|a, b| a.partial_cmp(b).unwrap());
        println!(
            "keyset n={n} load_s={load:.2} p50_ms={:.3} p99_ms={:.3}",
            samples[100], samples[198]
        );
    }
}
