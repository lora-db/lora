"""Streams pull rows in batches with the GIL released.

The first pull of a sort or aggregation does all of its work. Holding the
GIL (or the event loop) for it froze every other Python thread (or
coroutine) for the duration. Rows are now pulled in batches with the GIL
released, off the event loop for ``AsyncDatabase.stream``; the batching
must stay invisible: rows arrive in order, and a mutating stream still
commits only on the pull past its last row.
"""

from __future__ import annotations

import asyncio
import threading
import time

from lora_python import AsyncDatabase, Database

# Seconds of work in a release build: a three-way cartesian product.
SLOW = "MATCH (a:N), (b:N), (c:N) RETURN count(*) AS c"
SLOW_NODES = 400


def seeded(n: int) -> Database:
    db = Database()
    db.execute("UNWIND range(1, $n) AS i CREATE (:N {i: i})", {"n": n})
    return db


def test_slow_first_pull_releases_the_gil() -> None:
    db = seeded(SLOW_NODES)
    stop = threading.Event()
    longest_stall = 0.0

    def ticker() -> None:
        nonlocal longest_stall
        last = time.perf_counter()
        while not stop.is_set():
            time.sleep(0.001)
            now = time.perf_counter()
            longest_stall = max(longest_stall, now - last)
            last = now

    thread = threading.Thread(target=ticker)
    thread.start()
    time.sleep(0.05)
    try:
        rows = list(db.stream(SLOW))
    finally:
        stop.set()
        thread.join()
    assert rows == [{"c": SLOW_NODES**3}]
    # With the GIL held for the pull the ticker stalls for all of it.
    assert longest_stall < 0.5, f"other thread stalled {longest_stall:.2f}s during the pull"


def test_rows_cross_batch_boundaries_in_order() -> None:
    db = seeded(1000)
    got = [row["i"] for row in db.stream("MATCH (n:N) RETURN n.i AS i ORDER BY i")]
    assert got == list(range(1, 1001))
    assert db.execute("MATCH (n:N) RETURN n.i AS i ORDER BY i")["rows"] == [
        {"i": i} for i in got
    ]


def test_mutating_stream_closed_after_its_last_row_rolls_back() -> None:
    db = Database()
    stream = db.stream("UNWIND range(1, 5) AS i CREATE (:W {i: i}) RETURN i")
    for _ in range(5):
        next(stream)
    stream.close()
    assert db.execute("MATCH (w:W) RETURN count(w) AS c")["rows"] == [{"c": 0}]

    assert len(list(db.stream("UNWIND range(1, 5) AS i CREATE (:W {i: i}) RETURN i"))) == 5
    assert db.execute("MATCH (w:W) RETURN count(w) AS c")["rows"] == [{"c": 5}]


async def test_async_stream_pulls_off_the_event_loop() -> None:
    db = AsyncDatabase(seeded(SLOW_NODES))
    ticks = 0
    stop = False

    async def ticker() -> None:
        nonlocal ticks
        while not stop:
            await asyncio.sleep(0)
            ticks += 1

    task = asyncio.create_task(ticker())
    rows = [row async for row in db.stream(SLOW)]
    stop = True
    await task
    assert rows == [{"c": SLOW_NODES**3}]
    assert ticks > 100, f"event loop ticked {ticks} times during the pull"


async def test_async_stream_cancelled_mid_pull_closes_the_stream() -> None:
    inner = seeded(SLOW_NODES)
    db = AsyncDatabase(inner)

    async def consume() -> None:
        async for _ in db.stream(SLOW):
            pass

    task = asyncio.create_task(consume())
    await asyncio.sleep(0.05)
    task.cancel()
    try:
        await task
    except asyncio.CancelledError:
        pass
    # The database stays usable, for reads and writes.
    inner.execute("CREATE (:After)")
    assert inner.execute("MATCH (a:After) RETURN count(a) AS c")["rows"] == [{"c": 1}]
