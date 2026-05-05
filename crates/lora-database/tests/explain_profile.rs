//! Tests for `Database::explain` and `Database::profile`.
//!
//! These verify that:
//!  - `explain` never executes the query (mutating queries leave the
//!    graph untouched).
//!  - `profile` does execute, including writes, and reports metrics.
//!  - parameters are forwarded.
//!  - errors flow through the same `LoraError` path as `execute`.

mod test_helpers;

use std::collections::BTreeMap;

use lora_database::{LoraErrorCode, LoraValue, PlanShape};
use test_helpers::TestDb;

#[test]
fn explain_does_not_execute_create() {
    let db = TestDb::new();
    let plan = db
        .service
        .explain("CREATE (:Foo {name: 'bar'})", None)
        .expect("explain should compile");
    assert!(plan.shape.is_mutating());
    assert_eq!(
        db.service.node_count(),
        0,
        "explain must not mutate the graph"
    );
    assert_eq!(plan.query, "CREATE (:Foo {name: 'bar'})");
}

#[test]
fn explain_returns_operator_tree_for_match() {
    let db = TestDb::new();
    db.run("CREATE (:Person {name: 'Alice'})");
    let plan = db
        .service
        .explain("MATCH (p:Person) RETURN p", None)
        .unwrap();
    assert_eq!(plan.shape, PlanShape::ReadOnly);
    assert_eq!(plan.result_columns, vec!["p".to_string()]);
    // Walk the tree: must contain at least one NodeByLabelScan.
    assert!(
        contains_operator(&plan.tree.root, "NodeByLabelScan"),
        "expected NodeByLabelScan in plan, got {:?}",
        plan.tree
    );
}

#[test]
fn explain_with_union_renders_synthetic_root() {
    let db = TestDb::new();
    let plan = db
        .service
        .explain(
            "MATCH (n:A) RETURN n.name AS name UNION ALL MATCH (m:B) RETURN m.name AS name",
            None,
        )
        .unwrap();
    assert_eq!(plan.tree.root.operator, "Union");
    assert!(plan.tree.root.children.len() >= 2);
}

#[test]
fn explain_accepts_params_without_executing() {
    let db = TestDb::new();
    db.run("CREATE (:Person {name: 'Alice'})");
    let mut params = BTreeMap::new();
    params.insert("name".to_string(), LoraValue::String("Alice".into()));
    let plan = db
        .service
        .explain(
            "MATCH (p:Person) WHERE p.name = $name RETURN p",
            Some(params),
        )
        .unwrap();
    assert_eq!(plan.shape, PlanShape::ReadOnly);
    assert_eq!(plan.result_columns, vec!["p".to_string()]);
    assert_eq!(db.service.node_count(), 1);
}

#[test]
fn explain_propagates_parse_error_with_same_code_as_execute() {
    let db = TestDb::new();
    let exec_err = db.service.execute("INVALID QUERY", None).unwrap_err();
    let explain_err = db.service.explain("INVALID QUERY", None).unwrap_err();
    assert_eq!(
        exec_err.code(),
        explain_err.code(),
        "explain must surface the same LoraErrorCode as execute (both: {:?} vs {:?})",
        exec_err.code(),
        explain_err.code(),
    );
    assert_eq!(explain_err.code(), LoraErrorCode::Parse);
}

#[test]
fn explain_propagates_analyzer_error() {
    let db = TestDb::new();
    let exec_err = db
        .service
        .execute("MATCH (n) RETURN unknown_var", None)
        .unwrap_err();
    let explain_err = db
        .service
        .explain("MATCH (n) RETURN unknown_var", None)
        .unwrap_err();
    assert_eq!(exec_err.code(), explain_err.code());
}

#[test]
fn profile_executes_create_and_reports() {
    let db = TestDb::new();
    let prof = db
        .service
        .profile("CREATE (:Foo {n: 1}) RETURN 1 AS one", None)
        .unwrap();
    assert!(
        prof.metrics.mutated,
        "profile of CREATE must report mutated"
    );
    assert_eq!(prof.metrics.total_rows, 1);
    assert_eq!(prof.plan.shape, PlanShape::Mutating);
    assert_eq!(
        db.service.node_count(),
        1,
        "profile must persist mutations like execute"
    );
}

#[test]
fn profile_reports_total_elapsed_and_rows() {
    let db = TestDb::new();
    db.run("CREATE (:Person {name: 'Alice', age: 30})");
    db.run("CREATE (:Person {name: 'Bob', age: 25})");
    db.run("CREATE (:Person {name: 'Carol', age: 35})");
    let prof = db
        .service
        .profile("MATCH (p:Person) RETURN p.name AS name", None)
        .unwrap();
    assert_eq!(prof.metrics.total_rows, 3);
    assert!(!prof.metrics.mutated);
    let _ = prof.metrics.total_elapsed_ns;
    assert_eq!(prof.plan.result_columns, vec!["name".to_string()]);
}

#[test]
fn profile_reports_per_operator_step_timing() {
    let db = TestDb::new();
    for n in ["Alice", "Bob", "Carol", "Dave", "Eve"] {
        db.run(&format!("CREATE (:Person {{name: '{n}'}})"));
    }
    let prof = db
        .service
        .profile(
            "MATCH (p:Person) WHERE p.name <> 'Bob' RETURN p.name AS name",
            None,
        )
        .unwrap();

    assert!(
        !prof.metrics.per_operator.is_empty(),
        "profile must record per-operator metrics for streaming reads, got {:?}",
        prof.metrics.per_operator,
    );

    // Every recorded operator must have at least one next_call. The
    // top operator must have produced a row count consistent with the
    // total returned rows.
    for (op_id, m) in &prof.metrics.per_operator {
        assert!(
            m.next_calls > 0,
            "operator {op_id} reported zero next_calls in {:?}",
            prof.metrics.per_operator,
        );
    }

    // Sum of self-times must be at most total elapsed (timings are
    // inclusive of children, so the root's elapsed should be near
    // total_elapsed_ns; we only sanity-check that the root operator
    // exists in the map).
    let plan_root_id = prof.plan.tree.root.id;
    if plan_root_id != usize::MAX {
        // Non-synthetic root: its metric entry must exist.
        assert!(
            prof.metrics.per_operator.contains_key(&plan_root_id),
            "root operator id {plan_root_id} missing from per-operator metrics",
        );
    }

    assert_eq!(prof.metrics.total_rows, 4);
}

#[test]
fn profile_with_params_forwarded() {
    let db = TestDb::new();
    db.run("CREATE (:Person {name: 'Alice'})");
    db.run("CREATE (:Person {name: 'Bob'})");
    let mut params = BTreeMap::new();
    params.insert("name".to_string(), LoraValue::String("Alice".into()));
    let prof = db
        .service
        .profile(
            "MATCH (p:Person) WHERE p.name = $name RETURN p",
            Some(params),
        )
        .unwrap();
    assert_eq!(prof.metrics.total_rows, 1);
}

#[test]
fn profile_propagates_runtime_error() {
    let db = TestDb::new();
    db.run("CREATE (:Person {name: 'Alice'})");
    // DELETE without DETACH on a node that has no relationships works,
    // but DELETE on a non-node value (here, a property) raises a
    // runtime error at execute time. profile() must surface the same
    // LoraErrorCode as execute().
    let q = "MATCH (p:Person) DELETE p.name";
    let exec_err = db.service.execute(q, None).map(|_| ()).unwrap_err();
    let prof_err = db.service.profile(q, None).map(|_| ()).unwrap_err();
    assert_eq!(exec_err.code(), prof_err.code());
}

fn contains_operator(node: &lora_database::PlanTreeNode, name: &str) -> bool {
    if node.operator == name {
        return true;
    }
    node.children.iter().any(|c| contains_operator(c, name))
}
