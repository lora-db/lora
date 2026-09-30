//! A FULLTEXT index indexes each string of a list property, whether the
//! list exists before or after the index; other elements are skipped.

mod test_helpers;
use test_helpers::TestDb;

#[test]
fn fulltext_indexes_each_string_of_a_list_created_after_the_index() {
    let db = TestDb::new();
    db.run("CREATE FULLTEXT INDEX fa FOR (n:F) ON EACH [n.aka]");
    db.run("CREATE (:F {key: 'a', aka: ['Oslo summer', 'Øya']}), (:F {key: 'b', aka: ['Bergen']})");
    let q = |term: &str| {
        db.sorted_strings(
            &format!(
                "CALL db.index.fulltext.queryNodes('fa', '{term}') YIELD node RETURN node.key AS k"
            ),
            "k",
        )
    };
    assert_eq!(q("oslo"), vec!["a"]);
    assert_eq!(q("oya"), vec!["a"]);
    assert_eq!(q("bergen"), vec!["b"]);
    // Updating the list re-indexes it.
    db.run("MATCH (f:F {key: 'b'}) SET f.aka = ['Oslo fjord']");
    assert_eq!(q("oslo"), vec!["a", "b"]);
    assert!(q("bergen").is_empty());
}

#[test]
fn fulltext_indexes_a_list_that_exists_before_the_index() {
    let db = TestDb::new();
    db.run("CREATE (:F {key: 'a', aka: ['Oslo summer', 42]})");
    db.run("CREATE FULLTEXT INDEX fa FOR (n:F) ON EACH [n.aka, n.name]");
    let got = db.sorted_strings(
        "CALL db.index.fulltext.queryNodes('fa', 'summer') YIELD node RETURN node.key AS k",
        "k",
    );
    assert_eq!(got, vec!["a"]);
}
