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
from typing import Any, Callable, Mapping, Optional, TypeVar

from ._native import Database as _Database
from .types import LoraParams, QueryResult

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
    """asyncio-compatible handle to an in-memory Lora database.

    All methods delegate to the sync ``Database`` on a worker thread so
    the event loop is never blocked by engine work. Methods are coroutines
    so normal async usage looks like::

        db = await AsyncDatabase.create()
        result = await db.execute("MATCH (n) RETURN n")

    Concurrency: a single ``AsyncDatabase`` wraps a single ``Database``;
    concurrent ``execute`` coroutines serialise on the underlying engine's
    mutex but do not block the event loop while waiting.
    """

    __slots__ = ("_inner",)

    def __init__(self, inner: _Database) -> None:
        self._inner = inner

    @classmethod
    async def create(cls) -> "AsyncDatabase":
        """Construct a fresh in-memory database. Async for API symmetry."""
        # Construction is cheap (Arc::new + Mutex::new) so we stay on-thread.
        return cls(_Database())

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

    async def clear(self) -> None:
        """Drop every node and relationship."""
        await _to_thread(self._inner.clear)

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
