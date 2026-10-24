//! Two planner fixes found in an ingest pipeline:
//!
//! * G-67: `$p IS NULL OR n.k IN $p` folds at plan time for the given
//!   parameters, so the `IN` is sought like any other (or the disjunction
//!   is dropped), instead of seeking an unselective conjunct and holding
//!   every candidate until the filter drops it. The plan is cached per
//!   parameter nullness, so the same text with a null parameter still
//!   answers correctly.
//! * G-66: a chain anchored at both ends by a key starts from the end
//!   whose relationship fans out least, not from the one whose seek merely
//!   looks a row cheaper.

mod test_helpers;

use std::collections::BTreeMap;

use lora_database::{
    ExecuteOptions, LoraValue, PlanTreeNode, QueryResult, ResultFormat, TransactionMode,
};
use test_helpers::TestDb;

fn s(v: &str) -> LoraValue {
    LoraValue::String(v.to_string())
}

fn list(items: Vec<LoraValue>) -> LoraValue {
    LoraValue::List(items)
}

fn params(pairs: &[(&str, LoraValue)]) -> BTreeMap<String, LoraValue> {
    pairs
        .iter()
        .map(|(k, v)| (k.to_string(), v.clone()))
        .collect()
}

fn find<'a>(node: &'a PlanTreeNode, op: &str, out: &mut Vec<&'a PlanTreeNode>) {
    if node.operator == op {
        out.push(node);
    }
    for child in &node.children {
        find(child, op, out);
    }
}

fn ops<'a>(node: &'a PlanTreeNode, op: &str) -> Vec<&'a PlanTreeNode> {
    let mut out = Vec::new();
    find(node, op, &mut out);
    out
}

fn seed_entities(db: &TestDb) {
    db.run("CREATE CONSTRAINT ek FOR (e:Entity) REQUIRE e.key IS UNIQUE");
    db.run("CREATE INDEX ec FOR (e:Entity) ON (e.country)");
    db.run("CREATE INDEX ekind FOR (e:Entity) ON (e.kind)");
    db.run(
        "UNWIND range(1, 2000) AS i CREATE (:Entity {key: 'v' + toString(i), kind: 'venue', country: 'NL'})",
    );
    db.run("CREATE (:Entity {key: 'c1', kind: 'country', country: 'NL'})");
    db.run("CREATE (:Entity {key: 'b1', kind: 'country', country: 'BE'})");
}

const FIND: &str = "UNWIND $values AS v MATCH (e:Entity {country: v}) \
    WHERE $kinds IS NULL OR e.kind IN $kinds RETURN e.key AS key ORDER BY key";

fn keys(db: &TestDb, p: BTreeMap<String, LoraValue>) -> Vec<String> {
    db.run_with_params(FIND, p)
        .iter()
        .map(|r| r["key"].as_str().unwrap().to_string())
        .collect()
}

#[test]
fn a_known_parameter_seeks_the_in_list() {
    let db = TestDb::new();
    seed_entities(&db);
    let p = params(&[
        ("values", list(vec![s("NL"); 50])),
        ("kinds", list(vec![s("country")])),
    ]);
    // It plans as the same query without the disjunction would.
    let plain = "UNWIND $values AS v MATCH (e:Entity {country: v}) \
        WHERE e.kind IN $kinds RETURN e.key AS key ORDER BY key";
    let seeks = |q: &str| -> Vec<(Option<String>, Option<String>)> {
        let plan = db.service.explain(q, Some(p.clone())).unwrap().tree.root;
        ops(&plan, "NodeByPropertyScan")
            .iter()
            .map(|n| {
                (
                    n.details.get("key").cloned(),
                    n.details.get("mode").cloned(),
                )
            })
            .collect()
    };
    assert_eq!(seeks(FIND), seeks(plain));
    // No filter is left holding the disjunction.
    let plan = db.service.explain(FIND, Some(p.clone())).unwrap().tree.root;
    let filters = ops(&plan, "Filter");
    assert!(
        filters
            .iter()
            .all(|f| !f.details.values().any(|d| d.contains("IsNull"))),
        "{plan:#?}"
    );
    assert_eq!(keys(&db, p), vec!["c1"; 50]);
}

#[test]
fn the_same_text_answers_for_null_and_non_null_parameters() {
    let db = TestDb::new();
    seed_entities(&db);
    let values = list(vec![s("NL"), s("BE")]);
    // Alternate, so each call may meet the plan cached for the other.
    for _ in 0..3 {
        let picked = keys(
            &db,
            params(&[
                ("values", values.clone()),
                ("kinds", list(vec![s("country")])),
            ]),
        );
        assert_eq!(picked, vec!["b1", "c1"]);
        let all = keys(
            &db,
            params(&[("values", values.clone()), ("kinds", LoraValue::Null)]),
        );
        assert_eq!(all.len(), 2002);
        let none = keys(
            &db,
            params(&[("values", values.clone()), ("kinds", list(vec![]))]),
        );
        assert!(none.is_empty());
    }
    // `IS NOT NULL` and `NOT` fold as well.
    let q = "MATCH (e:Entity) WHERE NOT ($k IS NOT NULL) OR e.key = $k RETURN count(e) AS c";
    assert_eq!(db.run_with_params(q, params(&[("k", s("c1"))]))[0]["c"], 1);
    assert_eq!(
        db.run_with_params(q, params(&[("k", LoraValue::Null)]))[0]["c"],
        2002
    );
}

#[test]
fn a_transaction_folds_too() {
    let db = TestDb::new();
    seed_entities(&db);
    let mut tx = db
        .service
        .begin_transaction(TransactionMode::ReadOnly)
        .unwrap();
    for (kinds, expected) in [(list(vec![s("country")]), 1usize), (LoraValue::Null, 2001)] {
        let result = tx
            .execute_with_params(
                FIND,
                Some(ExecuteOptions {
                    format: ResultFormat::RowArrays,
                }),
                params(&[("values", list(vec![s("NL")])), ("kinds", kinds)]),
            )
            .unwrap();
        let QueryResult::RowArrays(rows) = result else {
            panic!("asked for row arrays");
        };
        assert_eq!(rows.rows.len(), expected);
    }
}

#[test]
fn a_chain_anchored_at_both_ends_expands_from_the_narrow_end() {
    let db = TestDb::new();
    db.run("CREATE CONSTRAINT e_key FOR (e:Entity) REQUIRE e.key IS UNIQUE");
    db.run("CREATE CONSTRAINT li_key FOR (l:LiveInstance) REQUIRE l.key IS UNIQUE");
    db.run("CREATE (:LiveInstance {key: 'x'})");
    db.run(
        "MATCH (l:LiveInstance {key: 'x'}) UNWIND range(1, 3000) AS i \
         CREATE (:Entity {key: 'e' + toString(i)})-[:PROMOTED_TO]->(l)",
    );
    db.run("CREATE (:Entity {key: 'loose'})");
    let both = "UNWIND $keys AS k MATCH (e:Entity {key: k})-[:PROMOTED_TO]->(:LiveInstance {key: 'x'}) RETURN k ORDER BY k";
    let split = "UNWIND $keys AS k MATCH (e:Entity {key: k}) MATCH (e)-[:PROMOTED_TO]->(i:LiveInstance) WHERE i.key = 'x' RETURN k ORDER BY k";
    let p = params(&[(
        "keys",
        list(vec![s("e1"), s("e2999"), s("loose"), s("missing")]),
    )]);
    let plan = db.service.explain(both, Some(p.clone())).unwrap().tree.root;
    let expands = ops(&plan, "Expand");
    assert_eq!(expands.len(), 1);
    assert_eq!(
        expands[0].details.get("direction").map(String::as_str),
        Some("->"),
        "starts at the Entity end: {plan:#?}"
    );
    let seeks = ops(&plan, "NodeByPropertyScan");
    assert_eq!(
        seeks[0].details.get("labels").map(String::as_str),
        Some("Entity")
    );
    let a = db.run_with_params(both, p.clone());
    let b = db.run_with_params(split, p);
    assert_eq!(a, b);
    assert_eq!(a.len(), 2);
}
