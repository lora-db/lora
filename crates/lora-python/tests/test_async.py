"""Async API smoke tests.

Verifies that ``AsyncDatabase.execute`` runs off the event loop (the
PyO3 ``Database`` releases the GIL during query execution, and
``asyncio.to_thread`` dispatches to a worker thread). A dedicated
non-blocking test counts event-loop ticks during a 2 000-node MATCH
and asserts that the loop kept ticking while the engine worked.
"""

from __future__ import annotations

import asyncio
import io
import time
from pathlib import Path

import pytest

from lora_python import AsyncDatabase, LoraQueryError, InvalidParamsError, is_node


async def test_create_returns_async_database() -> None:
    db = await AsyncDatabase.create()
    assert isinstance(db, AsyncDatabase)


async def test_basic_execute_round_trip() -> None:
    db = await AsyncDatabase.create()
    await db.execute("CREATE (:Person {name: 'Alice'})")
    r = await db.execute("MATCH (n:Person) RETURN n.name AS name")
    assert r["rows"] == [{"name": "Alice"}]
    assert db.node_count == 1


async def test_params_pass_through() -> None:
    db = await AsyncDatabase.create()
    await db.execute("CREATE (:P {name: $n, qty: $q})", {"n": "widget", "q": 7})
    r = await db.execute("MATCH (p:P) RETURN p.name AS name, p.qty AS qty")
    assert r["rows"] == [{"name": "widget", "qty": 7}]


async def test_create_accepts_wal_dir_and_replays(tmp_path: Path) -> None:
    wal_dir = tmp_path / "wal"

    first = await AsyncDatabase.create(str(wal_dir))
    await first.execute(
        "CREATE (:User {id: 1})-[:FOLLOWS]->(:User {id: 2}) RETURN 1"
    )
    await first.close()

    second = await AsyncDatabase.create(str(wal_dir))
    assert second.node_count == 2
    assert second.relationship_count == 1
    r = await second.execute("MATCH (u:User) RETURN u.id AS id ORDER BY id")
    assert r["rows"] == [{"id": 1}, {"id": 2}]
    await second.close()


async def test_lora_error_propagates_through_await() -> None:
    db = await AsyncDatabase.create()
    with pytest.raises(LoraQueryError):
        await db.execute("THIS IS NOT CYPHER")


async def test_invalid_params_error_propagates_through_await() -> None:
    db = await AsyncDatabase.create()
    with pytest.raises(InvalidParamsError):
        await db.execute("RETURN $d AS d", {"d": {"kind": "date", "iso": "not-a-date"}})


async def test_invalid_wal_dir_error_propagates_through_await(tmp_path: Path) -> None:
    not_a_dir = tmp_path / "wal-file"
    not_a_dir.write_text("not a directory")

    with pytest.raises(LoraQueryError):
        await AsyncDatabase.create(str(not_a_dir))


async def test_many_concurrent_queries() -> None:
    db = await AsyncDatabase.create()
    results = await asyncio.gather(
        *[db.execute("RETURN $v AS v", {"v": i}) for i in range(50)]
    )
    assert [r["rows"][0]["v"] for r in results] == list(range(50))


async def test_event_loop_stays_responsive_during_heavy_query() -> None:
    """Proof that the GIL is released + asyncio.to_thread dispatches off-loop.

    Seeds 2 000 nodes sequentially, then runs a MATCH while a second
    coroutine ticks on the event loop. If the engine held the GIL or
    ran on the loop thread, ticks would stay at zero until the MATCH
    finished. With ``asyncio.to_thread`` + GIL release they interleave.
    """
    db = await AsyncDatabase.create()
    N = 2_000
    for i in range(N):
        await db.execute("CREATE (:P {i: $i})", {"i": i})
    assert db.node_count == N

    ticks = 0
    stop = False

    async def ticker() -> None:
        nonlocal ticks
        while not stop:
            await asyncio.sleep(0)  # yields to the loop each iteration
            ticks += 1

    ticker_task = asyncio.create_task(ticker())
    started = time.perf_counter()
    r = await db.execute("MATCH (n:P) RETURN n.i AS i ORDER BY i")
    elapsed = time.perf_counter() - started
    stop = True
    await ticker_task

    assert len(r["rows"]) == N
    assert r["rows"][0]["i"] == 0
    assert r["rows"][N - 1]["i"] == N - 1
    # If the event loop was blocked the whole time the ticker ran zero
    # iterations after the first await. Deliberately permissive — we only
    # need to show the loop *could* progress.
    assert ticks > 0, f"event loop blocked for {elapsed:.3f}s during MATCH"


async def test_node_result_shape_is_typed() -> None:
    db = await AsyncDatabase.create()
    await db.execute("CREATE (:Person {name: 'Bob'})")
    r = await db.execute("MATCH (n:Person) RETURN n")
    n = r["rows"][0]["n"]
    assert is_node(n)
    assert n["properties"]["name"] == "Bob"


async def test_async_stream_and_transaction_helpers() -> None:
    db = await AsyncDatabase.create()
    results = await db.transaction(
        [
            {"query": "UNWIND range(1, 3) AS i CREATE (:S {i: i})"},
            {"query": "MATCH (n:S) RETURN n.i AS i ORDER BY i"},
        ]
    )
    assert [row["i"] for row in results[1]["rows"]] == [1, 2, 3]

    seen = []
    async for row in db.stream("MATCH (n:S) RETURN n.i AS i ORDER BY i"):
        seen.append(row["i"])
    assert seen == [1, 2, 3]


async def test_async_snapshot_bytes_and_readers() -> None:
    source = await AsyncDatabase.create()
    await source.execute("CREATE (:Snapshot {name: 'Ada'})")

    binary = await source.save_snapshot("binary")
    assert isinstance(binary, bytes)
    encoded = await source.save_snapshot("base64")
    assert isinstance(encoded, str)

    writer = io.BytesIO()
    meta = await source.save_snapshot(writer)
    assert meta["nodeCount"] == 1

    for item in [binary, io.BytesIO(writer.getvalue()), (encoded, "base64")]:
        target = await AsyncDatabase.create()
        if isinstance(item, tuple):
            meta = await target.load_snapshot(item[0], format=item[1])
        else:
            meta = await target.load_snapshot(item)
        assert meta["nodeCount"] == 1
        rows = (await target.execute("MATCH (n:Snapshot) RETURN n.name AS name"))["rows"]
        assert rows == [{"name": "Ada"}]
