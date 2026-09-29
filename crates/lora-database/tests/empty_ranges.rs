//! Range predicates whose bounds cannot match (`x > 5 AND x < 5`,
//! `x >= 9 AND x <= 3`) return no rows through a RANGE index. They used to
//! reach `BTreeMap::range` with an empty range, which panics, and a panic
//! aborts a host process such as the Node binding.

mod test_helpers;
use test_helpers::TestDb;

#[test]
fn empty_and_inverted_ranges_on_an_indexed_property_return_no_rows() {
    let db = TestDb::new();
    db.run("CREATE RANGE INDEX FOR (d:Doc) ON (d.level)");
    db.run("UNWIND range(0, 3000) AS i CREATE (:Doc {level: i % 10})");
    for predicate in [
        "d.level > 5 AND d.level < 5",
        "d.level >= 5 AND d.level < 5",
        "d.level > 5 AND d.level <= 5",
        "d.level >= 9 AND d.level <= 3",
        "d.level > 9 AND d.level < 0",
    ] {
        let rows = db.run(&format!(
            "MATCH (d:Doc) WHERE {predicate} RETURN d.level AS l ORDER BY l DESC LIMIT 5"
        ));
        assert!(rows.is_empty(), "{predicate}: {rows:?}");
    }
    let rows = db.run("MATCH (d:Doc) WHERE d.level >= 5 AND d.level <= 5 RETURN count(d) AS c");
    assert_eq!(rows[0]["c"], 300);
}
