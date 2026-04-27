"""Basic sync usage example — mini social graph + typed query."""

from __future__ import annotations

from lora_python import Database, is_node


def main() -> None:
    db = Database.create()  # in-memory
    # db = Database.create("app", {"database_dir": "./data"})  # persistent: ./data/app.loradb

    db.execute(
        "CREATE (:Person {name: 'Alice', age: 30}), "
        "       (:Person {name: 'Bob', age: 28})"
    )
    db.execute(
        "MATCH (a:Person {name: $a}), (b:Person {name: $b}) "
        "CREATE (a)-[:FOLLOWS]->(b)",
        {"a": "Alice", "b": "Bob"},
    )

    res = db.execute("MATCH (n:Person) RETURN n")
    for row in res["rows"]:
        n = row["n"]
        if is_node(n):
            print(f"{n['properties']['name']} (age: {n['properties']['age']})")


if __name__ == "__main__":
    main()
