//! A RANGE index over a property holding both integers and floats answers
//! numeric comparisons by value, as an unindexed scan does.
//!
//! The index used to rank every integer below every float, so a float
//! bound skipped the integers above it (`n.age > 25.5` lost the 30) and an
//! integer bound skipped the floats below it (`n.age < 10` lost the 1.5),
//! and an ordered walk returned all integers before all floats.

mod test_helpers;
use test_helpers::TestDb;

fn people(indexed: bool) -> TestDb {
    let db = TestDb::new();
    if indexed {
        db.run("CREATE INDEX p_age FOR (n:P) ON (n.age)");
    }
    db.run(
        "CREATE (:P {k: 'int30', age: 30}), \
                (:P {k: 'float1.5', age: 1.5}), \
                (:P {k: 'int10', age: 10}), \
                (:P {k: 'float10', age: 10.0}), \
                (:P {k: 'float25.5', age: 25.5}), \
                (:P {k: 'int-3', age: -3}), \
                (:P {k: 'float-2.5', age: -2.5}), \
                (:P {k: 'str', age: 'x'}), \
                (:P {k: 'bool', age: true}), \
                (:P {k: 'none'})",
    );
    db
}

#[test]
fn mixed_number_ranges_match_an_unindexed_scan() {
    let queries = [
        "MATCH (n:P) WHERE n.age > 25.5 RETURN n.k AS k",
        "MATCH (n:P) WHERE n.age >= 25.5 RETURN n.k AS k",
        "MATCH (n:P) WHERE n.age < 10 RETURN n.k AS k",
        "MATCH (n:P) WHERE n.age <= 10 RETURN n.k AS k",
        "MATCH (n:P) WHERE n.age >= 10 RETURN n.k AS k",
        "MATCH (n:P) WHERE n.age > 10.0 RETURN n.k AS k",
        "MATCH (n:P) WHERE n.age <= 10.0 RETURN n.k AS k",
        "MATCH (n:P) WHERE n.age > -2.9 AND n.age < 11 RETURN n.k AS k",
        "MATCH (n:P) WHERE n.age = 10.0 RETURN n.k AS k",
        "MATCH (n:P) WHERE n.age = 10 RETURN n.k AS k",
        "MATCH (n:P {age: 10.0}) RETURN n.k AS k",
        "MATCH (n:P) WHERE n.age IN [30.0, 1.5] RETURN n.k AS k",
    ];
    let plain = people(false);
    let indexed = people(true);
    for q in queries {
        let want = plain.sorted_strings(q, "k");
        assert_eq!(indexed.sorted_strings(q, "k"), want, "{q}");
    }
    assert_eq!(
        indexed.sorted_strings("MATCH (n:P) WHERE n.age > 25.5 RETURN n.k AS k", "k"),
        vec!["int30"]
    );
    assert_eq!(
        indexed.sorted_strings("MATCH (n:P) WHERE n.age < 10 RETURN n.k AS k", "k"),
        vec!["float-2.5", "float1.5", "int-3"]
    );
}

#[test]
fn order_by_an_indexed_mixed_number_property_sorts_by_value() {
    for indexed in [false, true] {
        let db = people(indexed);
        for (q, want) in [
            (
                "MATCH (n:P) WHERE n.age > -100 RETURN n.age AS a ORDER BY n.age",
                vec!["-3", "-2.5", "1.5", "10", "10", "25.5", "30"],
            ),
            (
                "MATCH (n:P) WHERE n.age > -100 RETURN n.age AS a ORDER BY n.age DESC LIMIT 3",
                vec!["30", "25.5", "10"],
            ),
            (
                "MATCH (n:P) WHERE n.age < 100 RETURN n.age AS a ORDER BY n.age LIMIT 3",
                vec!["-3", "-2.5", "1.5"],
            ),
        ] {
            let got: Vec<String> = db
                .column(q, "a")
                .iter()
                .map(|v| v.as_f64().expect("a number").to_string())
                .collect();
            assert_eq!(got, want, "indexed: {indexed}: {q}");
        }
    }
}
