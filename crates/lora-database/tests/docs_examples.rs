use std::path::Path;

use lora_database::{Database, ExecuteOptions, InMemoryGraph, ResultFormat};

fn extract_query_examples(markdown: &str) -> Vec<String> {
    let mut examples = Vec::new();
    let mut rest = markdown;
    let marker = "<QueryCodeBlock code={String.raw`";

    while let Some(start) = rest.find(marker) {
        let body_start = start + marker.len();
        let body = &rest[body_start..];
        let Some(end) = body.find("`} />") else {
            panic!("unterminated QueryCodeBlock in examples documentation");
        };
        examples.push(body[..end].to_string());
        rest = &body[end + "`} />".len()..];
    }

    examples
}

fn execute(db: &Database<InMemoryGraph>, query: &str) {
    db.execute(
        query,
        Some(ExecuteOptions {
            format: ResultFormat::Rows,
        }),
    )
    .unwrap_or_else(|err| panic!("documentation query failed:\n{query}\n\nerror: {err}"));
}

#[test]
fn query_examples_run_against_documented_seed_graph() {
    let docs_path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../apps/loradb.com/docs/queries/examples.md");
    let markdown = std::fs::read_to_string(&docs_path).unwrap_or_else(|err| {
        panic!("failed to read {}: {err}", docs_path.display());
    });
    let examples = extract_query_examples(&markdown);

    assert!(
        examples.len() > 20,
        "expected the examples page to contain runnable QueryCodeBlock snippets"
    );

    let seed = &examples[0];
    execute(&Database::in_memory(), seed);

    for (index, query) in examples.iter().enumerate().skip(1) {
        let db = Database::in_memory();
        execute(&db, seed);
        db.execute(
            query,
            Some(ExecuteOptions {
                format: ResultFormat::Rows,
            }),
        )
        .unwrap_or_else(|err| {
            panic!(
                "documentation query #{index} failed against the seed graph:\n{query}\n\nerror: {err}"
            )
        });
    }
}
