"""Type stubs for the PyO3 extension module.

Mirrors what the native ``_native`` module exposes. The high-level
``lora_python`` package re-exports these with richer types from
``lora_python.types``.
"""

from os import PathLike
from typing import Any, BinaryIO, Iterable, Iterator, Literal, Mapping, Optional, overload

from .types import QueryResult, SnapshotMeta

SnapshotPath = str | PathLike[str]
SnapshotBytes = bytes | bytearray | memoryview
SnapshotLoadSource = SnapshotPath | SnapshotBytes | BinaryIO

__version__: str

class LoraError(Exception):
    """Base class for Lora engine errors."""

class LoraQueryError(LoraError):
    """Parse / analyze / execute failure."""

class InvalidParamsError(LoraError):
    """A parameter value could not be mapped to a Lora value."""

class Database:
    """Lora graph database (sync, PyO3)."""

    def __init__(self, wal_dir: Optional[str] = None) -> None: ...
    @staticmethod
    def create(wal_dir: Optional[str] = None) -> "Database": ...
    def execute(
        self,
        query: str,
        params: Optional[Mapping[str, Any]] = None,
    ) -> QueryResult: ...
    def stream(
        self,
        query: str,
        params: Optional[Mapping[str, Any]] = None,
    ) -> Iterator[Mapping[str, Any]]: ...
    def transaction(
        self,
        statements: Iterable[Mapping[str, Any]],
        mode: str = "read_write",
    ) -> list[QueryResult]: ...
    def clear(self) -> None: ...
    def close(self) -> None: ...
    @property
    def node_count(self) -> int: ...
    @property
    def relationship_count(self) -> int: ...
    @overload
    def save_snapshot(self) -> bytes: ...
    @overload
    def save_snapshot(self, target: Literal["binary", "bytes"]) -> bytes: ...
    @overload
    def save_snapshot(self, target: Literal["base64"]) -> str: ...
    @overload
    def save_snapshot(self, target: BinaryIO) -> SnapshotMeta: ...
    @overload
    def save_snapshot(self, target: SnapshotPath, format: None = None) -> SnapshotMeta: ...
    def save_snapshot(self, target: Any = None, format: Optional[str] = None) -> Any: ...
    @overload
    def load_snapshot(self, source: SnapshotLoadSource, format: None = None) -> SnapshotMeta: ...
    @overload
    def load_snapshot(self, source: str | bytes, format: Literal["base64"]) -> SnapshotMeta: ...
    def load_snapshot(self, source: Any, format: Optional[str] = None) -> SnapshotMeta: ...
    def __repr__(self) -> str: ...

class QueryStream(Iterator[Mapping[str, Any]]):
    """Native pull-based query stream."""

    def columns(self) -> list[str]: ...
    def close(self) -> None: ...
