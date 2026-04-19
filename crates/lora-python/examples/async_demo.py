"""Async usage example — showcases the non-blocking ``AsyncDatabase``.

While the MATCH runs on a worker thread, the main coroutine keeps
counting seconds on the event loop. The two activities interleave, so
the engine never starves the event loop.
"""

from __future__ import annotations

import asyncio

from lora_python import AsyncDatabase, is_node


async def seed(db: AsyncDatabase, n: int) -> None:
    for i in range(n):
        await db.execute("CREATE (:P {i: $i})", {"i": i})


async def ticker(label: str, stop: asyncio.Event) -> int:
    ticks = 0
    while not stop.is_set():
        await asyncio.sleep(0.01)
        ticks += 1
    print(f"{label}: loop ticked {ticks}× during the query")
    return ticks


async def main() -> None:
    db = await AsyncDatabase.create()
    await seed(db, 2_000)

    stop = asyncio.Event()
    ticker_task = asyncio.create_task(ticker("main", stop))

    result = await db.execute("MATCH (n:P) RETURN n.i AS i ORDER BY i")
    stop.set()
    await ticker_task

    print(f"matched {len(result['rows'])} rows")
    last = result["rows"][-1]
    assert last["i"] == 1_999

    # Node-shaped result sanity check.
    await db.execute("CREATE (:Q {x: 1})")
    node_row = (await db.execute("MATCH (q:Q) RETURN q"))["rows"][0]
    q = node_row["q"]
    if is_node(q):
        print(f"sample node: labels={q['labels']} properties={q['properties']}")


if __name__ == "__main__":
    asyncio.run(main())
