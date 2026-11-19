//! A POINT index over WGS-84 points answers boxes and distances exactly as
//! a label scan does, near the poles and across the antimeridian too, and
//! a box around one country reads that country's cells, not every point.

mod test_helpers;
use std::time::Instant;

use test_helpers::TestDb;

/// Points on a skewed lattice over the globe, poles and ±180 included.
fn globe(indexed: bool) -> TestDb {
    let db = TestDb::new();
    if indexed {
        db.run("CREATE POINT INDEX g FOR (p:P) ON (p.at)");
    }
    db.run(
        "UNWIND range(0, 3599) AS i \
         CREATE (:P {k: i, at: point({longitude: -180.0 + (i * 37 % 3600) * 0.1, \
                                      latitude: -90.0 + (i * 53 % 1801) * 0.1})})",
    );
    db.run(
        "CREATE (:P {k: -1, at: point({longitude: 180.0, latitude: 0.0})}), \
                   (:P {k: -2, at: point({longitude: -180.0, latitude: 0.5})}), \
                   (:P {k: -3, at: point({longitude: 0.0, latitude: 90.0})}), \
                   (:P {k: -4, at: point({longitude: 120.0, latitude: -90.0})})",
    );
    db
}

const PREDICATES: &[&str] = &[
    // Distances in metres: small, across the antimeridian, at the poles,
    // and larger than the Earth.
    "geo.distance(p.at, point({longitude: 4.9, latitude: 52.4})) < 300000",
    "geo.distance(p.at, point({longitude: 179.9, latitude: 0.2})) <= 150000",
    "geo.distance(p.at, point({longitude: -179.95, latitude: -10.0})) < 2000000",
    "geo.distance(p.at, point({longitude: 10.0, latitude: 89.5})) < 500000",
    "geo.distance(p.at, point({longitude: -60.0, latitude: -88.0})) < 900000",
    "geo.distance(p.at, point({longitude: 0.0, latitude: 0.0})) < 30000000",
    // Boxes: a country, one across the antimeridian, the whole globe.
    "geo.within_bbox(p.at, point({longitude: 3.3, latitude: 50.7}), point({longitude: 7.2, latitude: 53.6}))",
    "geo.within_bbox(p.at, point({longitude: 170.0, latitude: -20.0}), point({longitude: -170.0, latitude: 20.0}))",
    "geo.within_bbox(p.at, point({longitude: -180.0, latitude: -90.0}), point({longitude: 180.0, latitude: 90.0}))",
];

#[test]
fn indexed_answers_match_the_scan() {
    let (indexed, plain) = (globe(true), globe(false));
    for predicate in PREDICATES {
        let q = format!("MATCH (p:P) WHERE {predicate} RETURN p.k AS k");
        let want = plain.sorted_ints(&q, "k");
        assert_eq!(indexed.sorted_ints(&q, "k"), want, "{predicate}");
    }
}

#[test]
fn a_country_box_does_not_read_the_continent() {
    let db = TestDb::new();
    db.run("CREATE POINT INDEX e FOR (e:E) ON (e.at)");
    let q = "MATCH (e:E) WHERE geo.within_bbox(e.at, point({longitude: 3.3, latitude: 50.7}), \
             point({longitude: 7.2, latitude: 53.6})) RETURN count(*) AS n";
    let time = |db: &TestDb| {
        db.run(q);
        (0..5)
            .map(|_| {
                let t = Instant::now();
                db.run(q);
                t.elapsed().as_secs_f64()
            })
            .fold(f64::MAX, f64::min)
    };
    // The same points in the box, with 20x more around them.
    db.run("UNWIND range(0, 1999) AS i CREATE (:E {at: point({longitude: 3.3 + (i % 40) * 0.09, latitude: 50.7 + (i / 40) * 0.05})})");
    let alone = time(&db);
    db.run("UNWIND range(0, 39999) AS i CREATE (:E {at: point({longitude: -10.0 + (i % 200) * 0.2, latitude: 36.0 + (i / 200) * 0.15})}) ");
    let crowded = time(&db);
    assert!(
        crowded < alone * 4.0 + 0.01,
        "box alone {alone:.5}s, inside 40k more points {crowded:.5}s"
    );
}
