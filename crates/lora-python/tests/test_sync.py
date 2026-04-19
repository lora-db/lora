"""Sync API smoke tests."""

from __future__ import annotations

import pytest

from lora_python import (
    Database,
    LoraQueryError,
    InvalidParamsError,
    cartesian,
    cartesian_3d,
    date,
    duration,
    is_node,
    is_path,
    is_point,
    is_relationship,
    is_temporal,
    wgs84,
    wgs84_3d,
)


def test_empty_match_returns_empty_rows() -> None:
    db = Database.create()
    r = db.execute("MATCH (n) RETURN n")
    assert r["rows"] == []
    assert r["columns"] == []


def test_create_and_return_node_with_properties() -> None:
    db = Database.create()
    db.execute("CREATE (:Person {name: 'Alice', age: 30})")
    assert db.node_count == 1

    r = db.execute("MATCH (n:Person) RETURN n")
    assert len(r["rows"]) == 1
    n = r["rows"][0]["n"]
    assert is_node(n)
    assert n["labels"] == ["Person"]
    assert n["properties"]["name"] == "Alice"
    assert n["properties"]["age"] == 30


def test_params_scalar_types() -> None:
    db = Database.create()
    db.execute(
        "CREATE (:Item {name: $n, qty: $q, active: $a, score: $s})",
        {"n": "widget", "q": 42, "a": True, "s": 1.5},
    )
    r = db.execute(
        "MATCH (i:Item) RETURN i.name AS name, i.qty AS qty, i.active AS active, i.score AS score",
    )
    assert r["rows"] == [{"name": "widget", "qty": 42, "active": True, "score": 1.5}]


def test_relationship_has_discriminator() -> None:
    db = Database.create()
    db.execute("CREATE (:A {n:1})-[:R {w:2}]->(:B {n:3})")
    r = db.execute("MATCH ()-[r:R]->() RETURN r")
    rel = r["rows"][0]["r"]
    assert is_relationship(rel)
    assert rel["type"] == "R"
    assert rel["properties"]["w"] == 2


def test_clear_empties_graph() -> None:
    db = Database.create()
    db.execute("CREATE (:X), (:Y)-[:R]->(:Z)")
    assert db.node_count == 3
    assert db.relationship_count == 1
    db.clear()
    assert db.node_count == 0
    assert db.relationship_count == 0


def test_roundtrips_mixed_list() -> None:
    db = Database.create()
    db.execute("CREATE (:N {xs: $xs})", {"xs": [1, "two", True, None]})
    rows = db.execute("MATCH (n:N) RETURN n.xs AS xs")["rows"]
    assert rows[0]["xs"] == [1, "two", True, None]


def test_roundtrips_nested_map() -> None:
    db = Database.create()
    db.execute("CREATE (:N {meta: $m})", {"m": {"a": 1, "b": {"c": "deep", "d": [True, False]}}})
    rows = db.execute("MATCH (n:N) RETURN n.meta AS m")["rows"]
    assert rows[0]["m"] == {"a": 1, "b": {"c": "deep", "d": [True, False]}}


def test_tagged_date_values() -> None:
    db = Database.create()
    db.execute("CREATE (:E {d: date('2025-03-14')})")
    rows = db.execute("MATCH (n:E) RETURN n.d AS d")["rows"]
    d = rows[0]["d"]
    assert is_temporal(d)
    assert d == {"kind": "date", "iso": "2025-03-14"}


def test_accepts_typed_temporal_params() -> None:
    db = Database.create()
    db.execute(
        "CREATE (:E {on: $d, span: $dur})",
        {"d": date("2025-01-15"), "dur": duration("P1M")},
    )
    rows = db.execute("MATCH (n:E) RETURN n.on AS on, n.span AS span")["rows"]
    assert rows[0]["on"] == {"kind": "date", "iso": "2025-01-15"}
    assert rows[0]["span"] == {"kind": "duration", "iso": "P1M"}


def test_tagged_point_values() -> None:
    db = Database.create()
    db.execute(
        "CREATE (:P {c: $c, g: $g})",
        {"c": cartesian(1.5, 2.5), "g": wgs84(4.9, 52.37)},
    )
    rows = db.execute("MATCH (n:P) RETURN n.c AS c, n.g AS g")["rows"]
    c = rows[0]["c"]
    g = rows[0]["g"]
    assert is_point(c) and is_point(g)
    # Cartesian 2D — srid, crs, x, y; no z; no geographic aliases.
    assert c["srid"] == 7203
    assert c["crs"] == "cartesian"
    assert c["x"] == pytest.approx(1.5)
    assert c["y"] == pytest.approx(2.5)
    assert "z" not in c
    assert "longitude" not in c
    # WGS-84 2D — srid, crs, x, y, and the geographic aliases.
    assert g["srid"] == 4326
    assert g["crs"] == "WGS-84-2D"
    assert g["x"] == pytest.approx(4.9)
    assert g["y"] == pytest.approx(52.37)
    assert g["longitude"] == pytest.approx(4.9)
    assert g["latitude"] == pytest.approx(52.37)
    assert "z" not in g
    assert "height" not in g


def test_tagged_point_values_3d() -> None:
    db = Database.create()
    db.execute(
        "CREATE (:P3 {c: $c, g: $g})",
        {
            "c": cartesian_3d(1.0, 2.0, 3.0),
            "g": wgs84_3d(4.89, 52.37, 15.0),
        },
    )
    rows = db.execute("MATCH (n:P3) RETURN n.c AS c, n.g AS g")["rows"]
    c = rows[0]["c"]
    g = rows[0]["g"]
    assert is_point(c) and is_point(g)
    # Cartesian 3D — includes z, no geographic aliases.
    assert c["srid"] == 9157
    assert c["crs"] == "cartesian-3D"
    assert c["z"] == pytest.approx(3.0)
    assert "longitude" not in c
    # WGS-84 3D — z, height, and the geographic aliases.
    assert g["srid"] == 4979
    assert g["crs"] == "WGS-84-3D"
    assert g["x"] == pytest.approx(4.89)
    assert g["z"] == pytest.approx(15.0)
    assert g["longitude"] == pytest.approx(4.89)
    assert g["latitude"] == pytest.approx(52.37)
    assert g["height"] == pytest.approx(15.0)


def test_point_from_cypher_constructor_round_trips() -> None:
    """3D points built inside Cypher emit the canonical external shape."""
    db = Database.create()
    rows = db.execute("RETURN point({x: 1.0, y: 2.0, z: 3.0}) AS p")["rows"]
    p = rows[0]["p"]
    assert is_point(p)
    assert p == {
        "kind": "point",
        "srid": 9157,
        "crs": "cartesian-3D",
        "x": 1.0,
        "y": 2.0,
        "z": 3.0,
    }


def test_path_invariant() -> None:
    db = Database.create()
    db.execute("CREATE (:A {n:1})-[:R]->(:B {n:2})")
    rows = db.execute("MATCH p = (:A)-[:R]->(:B) RETURN p")["rows"]
    p = rows[0]["p"]
    assert is_path(p)
    assert len(p["nodes"]) == len(p["rels"]) + 1


def test_parse_error_raises_lora_query_error() -> None:
    db = Database.create()
    with pytest.raises(LoraQueryError):
        db.execute("THIS IS NOT CYPHER")


def test_invalid_temporal_param_raises_invalid_params_error() -> None:
    db = Database.create()
    with pytest.raises(InvalidParamsError):
        db.execute("RETURN $d AS d", {"d": {"kind": "date", "iso": "not-a-date"}})


def test_temporal_now_functions_work() -> None:
    # `date()`, `datetime()`, … no-arg forms use the wall clock; they must
    # not raise inside the PyO3 extension.
    db = Database.create()
    r = db.execute(
        "RETURN date() AS d, datetime() AS dt, time() AS t, localdatetime() AS ldt, localtime() AS lt",
    )
    row = r["rows"][0]
    for key in ("d", "dt", "t", "ldt", "lt"):
        assert is_temporal(row[key]), f"{key} should be a tagged temporal dict"
    assert int(row["d"]["iso"][:4]) >= 2024


def test_repr_includes_counts() -> None:
    db = Database.create()
    db.execute("CREATE (:X), (:Y)-[:R]->(:Z)")
    assert repr(db) == "<lora_python.Database nodes=3 relationships=1>"
