"""Type stubs for the PyO3 extension module.

Mirrors what the native ``_native`` module exposes. The high-level
``lora_python`` package re-exports these with richer types from
``lora_python.types``.
"""

from typing import Any, Mapping, Optional

from .types import QueryResult

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
    def clear(self) -> None: ...
    def close(self) -> None: ...
    @property
    def node_count(self) -> int: ...
    @property
    def relationship_count(self) -> int: ...
    def save_snapshot(self, path: str) -> Mapping[str, Any]: ...
    def load_snapshot(self, path: str) -> Mapping[str, Any]: ...
    def __repr__(self) -> str: ...
