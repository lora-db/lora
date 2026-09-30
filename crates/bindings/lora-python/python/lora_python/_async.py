"""Async-compatible Database wrapper.

The PyO3 ``Database`` is synchronous — the engine itself is synchronous
Rust — but it releases the GIL while running a query, which means the
heavy work can safely be hoisted off the asyncio event-loop thread.

``AsyncDatabase`` does exactly that: each ``await db.execute(...)`` call
dispatches the sync ``Database.execute`` onto a worker thread via
``asyncio.to_thread`` on Python 3.9+, or the equivalent
``loop.run_in_executor`` polyfill on 3.8. The event loop stays free to
service other coroutines while the engine runs.

This is the pragmatic, well-understood pattern for async-wrapping a
CPU-bound Rust function in Python. It requires no unsafe lifetime
juggling and stays trivially debuggable.

A small or saturated executor delays writes but cannot deadlock them.
Every executor call is self-contained: ``execute`` and ``transaction``
take the writer lock, run, commit and release it within one native call
on one thread, with the GIL released, and never wait for another
executor task. The writer holding the lock is therefore always running,
however many others wait on executor threads. The one call that keeps
the lock across awaits, a mutating ``stream``, opens on a dedicated
thread and pulls its rows from the native stream's own thread.
"""

from __future__ import annotations

import asyncio
import contextvars
import functools
import sys
import threading
from typing import Any, AsyncIterator, Callable, Iterable, Mapping, Optional, TypeVar

from ._native import Database as _Database
from .types import LoraParams, QueryResult, SnapshotMeta

_T = TypeVar("_T")


# `asyncio.to_thread` landed in Python 3.9 (bpo-32309). Provide a direct
# equivalent on 3.8 so the non-blocking behaviour is identical: dispatch
# the call onto the running loop's default executor with the current
# context copied over, just like the CPython implementation.
if sys.version_info >= (3, 9):
    _to_thread = asyncio.to_thread  # type: ignore[attr-defined]
else:  # pragma: no cover — exercised in the 3.8 CI leg

    async def _to_thread(
        func: Callable[..., _T], /, *args: Any, **kwargs: Any
    ) -> _T:
        loop = asyncio.get_running_loop()
        ctx = contextvars.copy_context()
        return await loop.run_in_executor(
            None, functools.partial(ctx.run, func, *args, **kwargs)
        )


def _hand_over(future: "asyncio.Future[Any]", stream: Any, error: Optional[BaseException]) -> None:
    """Settle an opening stream's future on the loop (see ``_open_stream``)."""
    if future.cancelled():
        if stream is not None:
            stream.close()  # nobody will iterate it: roll back, free the lock
    elif error is not None:
        future.set_exception(error)
    else:
        future.set_result(stream)


def _open_stream(
    inner: _Database,
    query: str,
    params: Optional[dict],
    loop: asyncio.AbstractEventLoop,
    opened: "asyncio.Future[Any]",
) -> None:
    """Open a mutating stream on a thread of its own.

    Opening waits (GIL released) for the writer lock, which another stream
    or writer may hold for a while. Waiting on the event loop blocked the
    loop, often the only thread that could finish the stream holding the
    lock: a deadlock. Waiting on the default executor would park one of
    its few threads per waiting stream. This thread only waits for the
    lock, hands the open stream to the loop and exits.
    """
    stream, error = None, None
    try:
        stream = inner.stream(query, params)
    except BaseException as caught:  # noqa: BLE001 - handed to the awaiting coroutine
        error = caught
    try:
        loop.call_soon_threadsafe(_hand_over, opened, stream, error)
    except RuntimeError:
        # The loop closed meanwhile; nobody is waiting for the stream.
        if stream is not None:
            stream.close()


class AsyncDatabase:
    """asyncio-compatible handle to a Lora database.

    All methods delegate to the sync ``Database`` on a worker thread so
    the event loop is never blocked by engine work. Methods are coroutines
    so normal async usage looks like::

        db = await AsyncDatabase.create()
        result = await db.execute("MATCH (n) RETURN n")

    Concurrency: a single ``AsyncDatabase`` wraps a single ``Database``;
    concurrent read-only ``execute`` coroutines can share the underlying
    store read lock, while writes serialise without blocking the event loop.
    """

    __slots__ = ("_inner",)

    def __init__(self, inner: _Database) -> None:
        self._inner = inner

    @classmethod
    async def create(
        cls,
        database_name: Optional[str] = None,
        options: Optional[Mapping[str, Any]] = None,
    ) -> "AsyncDatabase":
        """Construct a database.

        ``database_name=None`` creates a fresh in-memory database.
        Passing a name opens or creates ``<database_dir>/<name>.loradb``.
        """
        if database_name is None and not options:
            return cls(_Database())
        return cls(await _to_thread(_Database.create, database_name, dict(options or {})))

    @classmethod
    async def open_wal(
        cls,
        wal_dir: str,
        options: Optional[Mapping[str, Any]] = None,
    ) -> "AsyncDatabase":
        """Open or create an explicit WAL-backed database."""
        return cls(await _to_thread(_Database.open_wal, wal_dir, dict(options or {})))

    async def close(self) -> None:
        """Release the native database handle."""
        await _to_thread(self._inner.close)

    async def execute(
        self,
        query: str,
        params: Optional[Mapping[str, Any]] = None,
    ) -> QueryResult:
        """Run a Lora query on a background thread.

        Returns ``{"columns": [...], "rows": [...]}``. Raises
        ``LoraQueryError`` on engine failure or ``InvalidParamsError``
        on a malformed parameter.
        """
        # The helper runs the callable on the loop's default
        # ThreadPoolExecutor. Since Database.execute releases the GIL,
        # other coroutines on the same event loop are free to progress.
        return await _to_thread(
            self._inner.execute,
            query,
            dict(params) if params is not None else None,
        )

    async def stream(
        self,
        query: str,
        params: Optional[Mapping[str, Any]] = None,
    ) -> AsyncIterator[Mapping[str, Any]]:
        """Yield query rows asynchronously.

        Rows are pulled one per iteration step, never ahead. A read-only
        stream reads a snapshot and never waits for a lock, so it opens
        on the event loop. A mutating stream takes the writer lock when it
        opens, so it opens on a thread of its own (``_open_stream``), and
        holds the lock until it is exhausted (commit) or closed early
        (rollback: ``break``, an exception, cancellation). Its rows come
        from the native stream's own thread, which holds the lock and never
        waits for the loop or the executor.
        """
        params = dict(params) if params is not None else None
        if self._inner._stream_is_mutating(query):
            loop = asyncio.get_running_loop()
            opened = loop.create_future()
            threading.Thread(
                target=_open_stream,
                args=(self._inner, query, params, loop, opened),
                name="lora-stream-open",
                daemon=True,
            ).start()
            stream = await opened
        else:
            stream = self._inner.stream(query, params)
        try:
            for row in stream:
                yield row
                await asyncio.sleep(0)
        finally:
            stream.close()

    async def transaction(
        self,
        statements: Iterable[Mapping[str, Any]],
        mode: str = "read_write",
    ) -> list[QueryResult]:
        """Execute a statement batch inside one native transaction."""
        normalized = []
        for statement in statements:
            item = dict(statement)
            if "params" in item and item["params"] is not None:
                item["params"] = dict(item["params"])
            normalized.append(item)
        return await _to_thread(self._inner.transaction, normalized, mode)

    async def clear(self) -> None:
        """Drop every node and relationship."""
        await _to_thread(self._inner.clear)

    async def save_snapshot(
        self,
        target: Any = None,
        format: Optional[str] = None,
        options: Optional[Mapping[str, Any]] = None,
    ) -> SnapshotMeta | bytes | str:
        """Save the graph to a snapshot path, bytes, base64, or writer.

        Path saves return ``SnapshotMeta``. ``"binary"`` / ``"bytes"`` return
        ``bytes``. ``"base64"`` returns text. A file-like writer receives the
        snapshot bytes and returns ``SnapshotMeta``.
        """
        return await _to_thread(
            self._inner.save_snapshot,
            target,
            format,
            dict(options) if options is not None else None,
        )

    async def load_snapshot(
        self,
        source: Any,
        format: Optional[str] = None,
        options: Optional[Mapping[str, Any]] = None,
    ) -> SnapshotMeta:
        """Replace the current graph state from a path, bytes, base64, or reader.

        Concurrent ``execute`` coroutines block on the store write lock
        until the load completes.
        """
        return await _to_thread(
            self._inner.load_snapshot,
            source,
            format,
            dict(options) if options is not None else None,
        )

    @property
    def node_count(self) -> int:
        return self._inner.node_count

    @property
    def relationship_count(self) -> int:
        return self._inner.relationship_count

    def __repr__(self) -> str:  # pragma: no cover — cosmetic
        return (
            f"<lora_python.AsyncDatabase "
            f"nodes={self._inner.node_count} "
            f"relationships={self._inner.relationship_count}>"
        )
