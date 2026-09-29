//! EXPLAIN estimates rows for index seeks the way the optimizer scores
//! them: range, text and point seeks as a fraction of their label.

mod test_helpers;
use lora_database::PlanTreeNode;
use test_helpers::TestDb;

fn find<'a>(node: &'a PlanTreeNode, operator: &str) -> Option<&'a PlanTreeNode> {
    if node.operator == operator {
        return Some(node);
    }
    node.children.iter().find_map(|c| find(c, operator))
}

#[test]
fn seeks_carry_estimated_rows() {
    let db = TestDb::new();
    db.run("CREATE RANGE INDEX FOR (n:Item) ON (n.v)");
    db.run("CREATE TEXT INDEX FOR (n:Item) ON (n.name)");
    db.run("UNWIND range(1, 1200) AS i CREATE (:Item {v: i, name: 'item ' + toString(i)})");
    let estimate = |query: &str, operator: &str| {
        let plan = db.service.explain(query, None).unwrap();
        find(&plan.tree.root, operator)
            .unwrap_or_else(|| panic!("{operator} in {query}"))
            .estimated_rows
    };
    assert_eq!(
        estimate(
            "MATCH (n:Item) WHERE n.v > 10 RETURN n",
            "NodeByPropertyRangeScan"
        ),
        Some(400)
    );
    assert_eq!(
        estimate(
            "MATCH (n:Item) WHERE n.v > 10 AND n.v < 20 RETURN n",
            "NodeByPropertyRangeScan"
        ),
        Some(300)
    );
    assert_eq!(
        estimate(
            "MATCH (n:Item) WHERE n.name CONTAINS 'x' RETURN n",
            "NodeByTextScan"
        ),
        Some(600)
    );
}
