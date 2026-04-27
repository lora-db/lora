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
"""

from __future__ import annotations

import asyncio
import contextvars
import functools
import sys
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
        Passing a name opens or creates ``<database_dir>/<name>.lora``.
        """
        if database_name is None:
            return cls(_Database())
        return cls(await _to_thread(_Database.create, database_name, dict(options or {})))

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

        The native binding keeps a Rust ``QueryStream`` open and pulls one
        row for each async iteration step.
        """
        stream = self._inner.stream(query, dict(params) if params is not None else None)
        for row in stream:
            yield row
            await asyncio.sleep(0)

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
    ) -> SnapshotMeta | bytes | str:
        """Save the graph to a snapshot path, bytes, base64, or writer.

        Path saves return ``SnapshotMeta``. ``"binary"`` / ``"bytes"`` return
        ``bytes``. ``"base64"`` returns text. A file-like writer receives the
        snapshot bytes and returns ``SnapshotMeta``.
        """
        return await _to_thread(self._inner.save_snapshot, target, format)

    async def load_snapshot(
        self,
        source: Any,
        format: Optional[str] = None,
    ) -> SnapshotMeta:
        """Replace the current graph state from a path, bytes, base64, or reader.

        Concurrent ``execute`` coroutines block on the store write lock
        until the load completes.
        """
        return await _to_thread(self._inner.load_snapshot, source, format)

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
