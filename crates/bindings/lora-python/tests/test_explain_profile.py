"""Tests for Database.explain and Database.profile."""

from __future__ import annotations

import pytest

from lora_python import Database, LoraQueryError


def test_explain_does_not_execute_mutating_query() -> None:
    db = Database.create()
    plan = db.explain("CREATE (:Foo {n: 1})")
    assert plan["shape"] == "mutating"
    assert db.node_count == 0


def test_explain_returns_plan_tree() -> None:
    db = Database.create()
    db.execute("CREATE (:Person {name: 'Alice'})")
    plan = db.explain("MATCH (p:Person) RETURN p")
    assert plan["shape"] == "readOnly"
    assert plan["result_columns"] == ["p"]
    assert plan["query"] == "MATCH (p:Person) RETURN p"
    assert "operator" in plan["tree"]


def test_explain_with_params_forwarded() -> None:
    db = Database.create()
    db.execute("CREATE (:Person {name: 'Alice'})")
    plan = db.explain(
        "MATCH (p:Person) WHERE p.name = $name RETURN p",
        {"name": "Alice"},
    )
    assert plan["shape"] == "readOnly"


def test_explain_surfaces_parse_error_like_execute() -> None:
    db = Database.create()
    with pytest.raises(LoraQueryError) as exec_exc:
        db.execute("INVALID")
    with pytest.raises(LoraQueryError) as explain_exc:
        db.explain("INVALID")
    # Both errors share the LORA_PARSE prefix.
    assert str(exec_exc.value).split(":")[0] == str(explain_exc.value).split(":")[0]


def test_profile_executes_mutating_query() -> None:
    db = Database.create()
    profile = db.profile("CREATE (:Foo {n: 1}) RETURN 1 AS one")
    assert profile["metrics"]["mutated"] is True
    assert profile["metrics"]["total_rows"] == 1
    assert db.node_count == 1


def test_profile_reports_per_operator_timing() -> None:
    db = Database.create()
    for name in ["Alice", "Bob", "Carol", "Dave"]:
        db.execute(f"CREATE (:Person {{name: '{name}'}})")
    profile = db.profile(
        "MATCH (p:Person) WHERE p.name <> 'Bob' RETURN p.name AS name"
    )
    assert profile["metrics"]["total_rows"] == 3
    assert profile["metrics"]["mutated"] is False
    per_op = profile["metrics"]["per_operator"]
    assert len(per_op) > 0
    for op_id, op in per_op.items():
        assert op["next_calls"] > 0


def test_profile_with_params_forwarded() -> None:
    db = Database.create()
    db.execute("CREATE (:Person {name: 'Alice'})")
    db.execute("CREATE (:Person {name: 'Bob'})")
    profile = db.profile(
        "MATCH (p:Person) WHERE p.name = $name RETURN p",
        {"name": "Alice"},
    )
    assert profile["metrics"]["total_rows"] == 1
