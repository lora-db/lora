"""Writers never wedge the process, however small the thread pool.

A read-write transaction, and a mutating stream, holds the database's
writer lock. If anything that waits for that lock also holds something
the lock holder needs (the GIL, the event loop, or the last free thread
of a bounded executor), the process hangs. Each scenario therefore runs
in a child process with a hard timeout, so a regression fails instead of
hanging the test run. The executors are deliberately tiny (2 threads)
and the writers many.
"""

from __future__ import annotations

import subprocess
import sys
import textwrap

TIMEOUT_S = 30


def run_child(source: str) -> str:
    try:
        done = subprocess.run(
            [sys.executable, "-c", textwrap.dedent(source)],
            capture_output=True,
            text=True,
            timeout=TIMEOUT_S,
        )
    except subprocess.TimeoutExpired as expired:  # pragma: no cover - regression path
        raise AssertionError(
            f"child hung for {TIMEOUT_S}s (deadlock); stderr: {expired.stderr!r}"
        ) from None
    assert done.returncode == 0, done.stderr
    return done.stdout.strip()


def test_async_writers_outnumber_a_two_thread_executor() -> None:
    out = run_child(
        """
        import asyncio
        from concurrent.futures import ThreadPoolExecutor
        from lora_python import AsyncDatabase

        async def main():
            asyncio.get_running_loop().set_default_executor(ThreadPoolExecutor(max_workers=2))
            db = await AsyncDatabase.create()

            async def batch(i):
                await db.transaction([
                    {"query": "CREATE (:N {i: $i})", "params": {"i": i}},
                    {"query": "MATCH (n:N {i: $i}) SET n.batched = true", "params": {"i": i}},
                ])

            async def auto(i):
                await db.execute("CREATE (:A {i: $i})", {"i": i})

            async def read(i):
                await db.execute("MATCH (n) RETURN count(n) AS c")

            async def stream(i):
                rows = []
                async for row in db.stream(
                    "UNWIND range(1, 3) AS x CREATE (:S {i: $i, x: x}) RETURN x", {"i": i}
                ):
                    rows.append(row["x"])
                    await asyncio.sleep(0.001)  # hold the writer lock across awaits
                assert rows == [1, 2, 3]

            async def abandoned(i):
                gen = db.stream("UNWIND range(1, 3) AS x CREATE (:Gone {x: x}) RETURN x")
                async for _ in gen:
                    await asyncio.sleep(0)
                    break
                await gen.aclose()  # rolls back and releases the lock

            async def clear_empty(i):
                other = await AsyncDatabase.create()
                await other.clear()

            jobs = (batch, auto, read, stream, abandoned, clear_empty)
            await asyncio.gather(*[job(i) for i in range(20) for job in jobs])
            counts = {}
            for label in ("N", "A", "S", "Gone"):
                r = await db.execute(f"MATCH (n:{label}) RETURN count(n) AS c")
                counts[label] = r["rows"][0]["c"]
            print(counts)

        asyncio.run(main())
        """
    )
    assert out == "{'N': 20, 'A': 20, 'S': 60, 'Gone': 0}"


def test_concurrent_async_mutating_streams() -> None:
    # Opening a second mutating stream on the event loop used to block the
    # loop on the writer lock, while only the loop could finish the first.
    out = run_child(
        """
        import asyncio
        from lora_python import AsyncDatabase

        async def main():
            db = await AsyncDatabase.create()

            async def stream(i):
                return [
                    row["x"]
                    async for row in db.stream(
                        "UNWIND range(1, 300) AS x CREATE (:S {i: $i, x: x}) RETURN x",
                        {"i": i},
                    )
                ]

            async def clear_later():
                await asyncio.sleep(0)
                await db.clear()

            results = await asyncio.gather(stream(1), stream(2), clear_later())
            assert results[0] == results[1] == list(range(1, 301))
            await db.clear()
            await asyncio.gather(*[stream(i) for i in range(10)])
            r = await db.execute("MATCH (n:S) RETURN count(n) AS c")
            print(r["rows"][0]["c"])

        asyncio.run(main())
        """
    )
    assert out == "3000"


def test_async_stream_errors_reach_the_awaiting_coroutine() -> None:
    out = run_child(
        """
        import asyncio
        from lora_python import AsyncDatabase, LoraQueryError

        async def main():
            db = await AsyncDatabase.create()
            try:
                async for _ in db.stream("THIS IS NOT CYPHER"):
                    pass
            except LoraQueryError:
                print("raised")
            # The lock is free again.
            await db.execute("CREATE (:After)")

        asyncio.run(main())
        """
    )
    assert out == "raised"


def test_threads_wait_for_a_stream_without_holding_the_gil() -> None:
    # A thread waiting for the writer lock with the GIL held froze the
    # thread whose open stream held the lock (it needs the GIL for rows).
    out = run_child(
        """
        import threading, time
        from concurrent.futures import ThreadPoolExecutor
        from lora_python import Database

        db = Database.create()
        opened = threading.Event()

        def streamer():
            stream = db.stream("UNWIND range(1, 3) AS x CREATE (:S {x: x}) RETURN x")
            opened.set()
            time.sleep(0.2)  # the others start waiting for the lock
            assert [row["x"] for row in stream] == [1, 2, 3]

        def writer(i):
            if i % 3 == 0:
                db.clear()
            elif i % 3 == 1:
                list(db.stream("CREATE (n:T) RETURN n"))
            else:
                db.transaction([{"query": "CREATE (:B)"}])

        holder = threading.Thread(target=streamer)
        holder.start()
        opened.wait()
        with ThreadPoolExecutor(max_workers=2) as pool:
            list(pool.map(writer, range(12)))
        holder.join()
        print("ok")
        """
    )
    assert out == "ok"


def test_stream_outlives_its_database() -> None:
    # The stream used to drop its database `Arc` before itself, so closing
    # the Database first freed the lock and store the stream still used
    # (SIGSEGV).
    out = run_child(
        """
        from lora_python import Database

        Q = "UNWIND range(1, 3) AS x CREATE (:S {x: x}) RETURN x"
        for i in range(200):
            db = Database.create()
            stream = db.stream(Q)
            next(stream)
            db.close()
            del db
            junk = [bytearray(64) for _ in range(1000)]
            if i % 2:
                assert [row["x"] for row in stream] == [2, 3]
            del stream  # dropped unfinished: rolls back
        for q in (Q, "UNWIND range(1, 3) AS x RETURN x"):
            db = Database.create()
            stream = db.stream(q)
            db.close()
            del db
            print([row["x"] for row in stream])
        """
    )
    assert out == "[1, 2, 3]\n[1, 2, 3]"


def test_mutating_stream_crosses_threads() -> None:
    # A mutating stream's writer lock guard must be released on the thread
    # that took it. Pulling on another thread used to abort the process,
    # and dropping it there leaked the lock, hanging every later write.
    out = run_child(
        """
        import gc, threading
        from lora_python import Database

        db = Database.create()
        Q = "UNWIND range(1, 3) AS x CREATE (:S {x: x}) RETURN x"

        def on_thread(fn):
            out = []
            t = threading.Thread(target=lambda: out.append(fn()))
            t.start()
            t.join()
            return out[0] if out else None

        # Opened here, pulled on another thread, exhausted: commits.
        stream = db.stream(Q)
        next(stream)
        print(on_thread(lambda: [row["x"] for row in stream]))

        # Opened here, dropped on another thread unfinished: rolls back.
        stream = db.stream(Q)
        next(stream)
        holder = [stream]
        del stream
        on_thread(holder.clear)
        gc.collect()

        # Opened on another thread, closed here.
        stream = on_thread(lambda: db.stream(Q))
        next(stream)
        stream.close()

        db.execute("CREATE (:After)")  # the lock is free
        print(db.execute("MATCH (n:S) RETURN count(n) AS c")["rows"][0]["c"])
        """
    )
    assert out == "[2, 3]\n3"


def test_async_mutating_stream_abandoned_while_opening() -> None:
    out = run_child(
        """
        import asyncio
        from lora_python import AsyncDatabase

        async def main():
            db = await AsyncDatabase.create()
            holder = db.stream("UNWIND range(1, 2) AS x CREATE (:H {x: x}) RETURN x")
            await holder.__anext__()  # holds the writer lock

            async def waiter():
                async for _ in db.stream("CREATE (:W) RETURN 1"):
                    pass

            task = asyncio.ensure_future(waiter())
            await asyncio.sleep(0.05)  # the waiter is opening, i.e. waiting for the lock
            task.cancel()
            await holder.aclose()  # rolls back and frees the lock
            await db.execute("CREATE (:After)")
            r = await db.execute("MATCH (n) RETURN labels(n)[0] AS l ORDER BY l")
            print([row["l"] for row in r["rows"]])

        asyncio.run(main())
        """
    )
    assert out == "['After']"
