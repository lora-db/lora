"""lora-python — typed Python bindings for the Lora graph engine.

Two public classes:

- ``Database``     : synchronous, PyO3-backed. Holds a Lora graph and
                     runs queries with the GIL released.
- ``AsyncDatabase``: pure-Python asyncio wrapper. Delegates to a
                     background thread via ``asyncio.to_thread`` so the
                     event loop stays responsive during heavier queries.

Both classes share the same public result / value / param shapes (see
``lora_python.types``) so switching between sync and async usage is a
method-name change, nothing else.

Example
-------
    >>> from lora_python import Database
    >>> db = Database.create()
    >>> db.execute("CREATE (:Person {name: $n})", {"n": "Alice"})
    >>> r = db.execute("MATCH (n:Person) RETURN n.name AS name")
    >>> r["rows"]
    [{'name': 'Alice'}]

Persistent:

    >>> db = Database.create("app", {"database_dir": "./data"})  # archive-backed persistent database

Async:

    >>> import asyncio
    >>> from lora_python import AsyncDatabase
    >>> async def main():
    ...     db = await AsyncDatabase.create()
    ...     await db.execute("CREATE (:Person {name: 'Alice'})")
    ...     return await db.execute("MATCH (n:Person) RETURN n.name AS name")
    >>> asyncio.run(main())["rows"]
    [{'name': 'Alice'}]
"""

from __future__ import annotations

from ._native import (
    Database,
    LoraError,
    LoraQueryError,
    InvalidParamsError,
    __version__,
)
from ._async import AsyncDatabase
from . import types
from .types import (
    LoraParam,
    LoraParams,
    LoraValue,
    QueryResult,
    LoraNode,
    LoraRelationship,
    LoraPath,
    LoraDate,
    LoraTime,
    LoraLocalTime,
    LoraDateTime,
    LoraLocalDateTime,
    LoraDuration,
    LoraPoint,
    LoraCartesianPoint,
    LoraCartesianPoint3D,
    LoraWgs84Point,
    LoraWgs84Point3D,
    LoraVector,
    LoraVectorCoordinateType,
    SnapshotMeta,
    date,
    time,
    localtime,
    datetime,
    localdatetime,
    duration,
    cartesian,
    cartesian_3d,
    wgs84,
    wgs84_3d,
    vector,
    is_node,
    is_relationship,
    is_path,
    is_point,
    is_temporal,
    is_vector,
)

__all__ = [
    "Database",
    "AsyncDatabase",
    "LoraError",
    "LoraQueryError",
    "InvalidParamsError",
    "__version__",
    "types",
    # re-exports
    "LoraParam",
    "LoraParams",
    "LoraValue",
    "QueryResult",
    "LoraNode",
    "LoraRelationship",
    "LoraPath",
    "LoraDate",
    "LoraTime",
    "LoraLocalTime",
    "LoraDateTime",
    "LoraLocalDateTime",
    "LoraDuration",
    "LoraPoint",
    "LoraCartesianPoint",
    "LoraCartesianPoint3D",
    "LoraWgs84Point",
    "LoraWgs84Point3D",
    "LoraVector",
    "LoraVectorCoordinateType",
    "SnapshotMeta",
    "date",
    "time",
    "localtime",
    "datetime",
    "localdatetime",
    "duration",
    "cartesian",
    "cartesian_3d",
    "wgs84",
    "wgs84_3d",
    "vector",
    "is_node",
    "is_relationship",
    "is_path",
    "is_point",
    "is_temporal",
    "is_vector",
]
