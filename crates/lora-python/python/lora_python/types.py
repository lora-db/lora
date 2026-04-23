"""Typed Python value model for lora-python.

Kept conceptually aligned with the shared TS contract used by
``lora-node`` / ``lora-wasm`` (see ``crates/shared-ts/types.ts``).
Python uses pragmatic representations:

- Scalars pass through as Python natives (``None``, ``bool``, ``int``,
  ``float``, ``str``).
- Lists and maps come back as ``list`` / ``dict``.
- Graph, temporal, and spatial values come back as ``TypedDict``s with
  a ``kind`` discriminator. They're plain dicts at runtime — no class
  wrappers to unpack — and narrow cleanly under ``typing.TYPE_CHECKING``.

If a caller wants structured objects, the ``is_node`` / ``is_temporal``
helpers make narrowing explicit.
"""

from __future__ import annotations

from typing import Any, List, Literal, Mapping, TypedDict, Union

# ---------------------------------------------------------------------------
# Forward-declare the recursive value union.
# ---------------------------------------------------------------------------

# ``LoraValue`` is recursive; PEP 604 union syntax works on 3.10+, but we
# fall back to ``Union`` for 3.8/3.9 support.
LoraValue = Union[
    None,
    bool,
    int,
    float,
    str,
    List["LoraValue"],
    Mapping[str, "LoraValue"],
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
    "LoraVector",
]

LoraParam = Union[
    None,
    bool,
    int,
    float,
    str,
    List["LoraParam"],
    Mapping[str, "LoraParam"],
    "LoraDate",
    "LoraTime",
    "LoraLocalTime",
    "LoraDateTime",
    "LoraLocalDateTime",
    "LoraDuration",
    "LoraPoint",
    "LoraVector",
]

LoraParams = Mapping[str, LoraParam]

# ---------------------------------------------------------------------------
# Structural values — TypedDicts with a `kind` discriminator.
# ---------------------------------------------------------------------------


class LoraNode(TypedDict):
    kind: Literal["node"]
    id: int
    labels: List[str]
    properties: Mapping[str, LoraValue]


class LoraRelationship(TypedDict):
    kind: Literal["relationship"]
    id: int
    startId: int
    endId: int
    type: str
    properties: Mapping[str, LoraValue]


class LoraPath(TypedDict):
    kind: Literal["path"]
    nodes: List[int]
    rels: List[int]


# ---------------------------------------------------------------------------
# Temporal — ISO-8601 tagged.
# ---------------------------------------------------------------------------


class LoraDate(TypedDict):
    kind: Literal["date"]
    iso: str


class LoraTime(TypedDict):
    kind: Literal["time"]
    iso: str


class LoraLocalTime(TypedDict):
    kind: Literal["localtime"]
    iso: str


class LoraDateTime(TypedDict):
    kind: Literal["datetime"]
    iso: str


class LoraLocalDateTime(TypedDict):
    kind: Literal["localdatetime"]
    iso: str


class LoraDuration(TypedDict):
    kind: Literal["duration"]
    iso: str


# ---------------------------------------------------------------------------
# Spatial
# ---------------------------------------------------------------------------


# ---------------------------------------------------------------------------
# Point variants.
#
# Python's ``TypedDict`` cannot model "required only on some variants" the
# way a TS discriminated union can, so we declare one dict per CRS and
# union them into ``LoraPoint``. Narrow via ``is_point`` + ``srid`` / ``crs``.
# ---------------------------------------------------------------------------


class LoraCartesianPoint(TypedDict):
    kind: Literal["point"]
    srid: Literal[7203]
    crs: Literal["cartesian"]
    x: float
    y: float


class LoraCartesianPoint3D(TypedDict):
    kind: Literal["point"]
    srid: Literal[9157]
    crs: Literal["cartesian-3D"]
    x: float
    y: float
    z: float


class LoraWgs84Point(TypedDict):
    kind: Literal["point"]
    srid: Literal[4326]
    crs: Literal["WGS-84-2D"]
    x: float
    y: float
    longitude: float
    latitude: float


class LoraWgs84Point3D(TypedDict):
    kind: Literal["point"]
    srid: Literal[4979]
    crs: Literal["WGS-84-3D"]
    x: float
    y: float
    z: float
    longitude: float
    latitude: float
    height: float


LoraPoint = Union[
    LoraCartesianPoint,
    LoraCartesianPoint3D,
    LoraWgs84Point,
    LoraWgs84Point3D,
]


# ---------------------------------------------------------------------------
# Vector
# ---------------------------------------------------------------------------


LoraVectorCoordinateType = Literal[
    "FLOAT64", "FLOAT32", "INTEGER", "INTEGER32", "INTEGER16", "INTEGER8"
]


class LoraVector(TypedDict):
    kind: Literal["vector"]
    dimension: int
    coordinateType: LoraVectorCoordinateType
    values: List[float]


# ---------------------------------------------------------------------------
# Query result
# ---------------------------------------------------------------------------


class QueryResult(TypedDict):
    columns: List[str]
    rows: List[Mapping[str, LoraValue]]


# ---------------------------------------------------------------------------
# Constructors for param-side temporal/spatial values.
# ---------------------------------------------------------------------------


def date(iso: str) -> LoraDate:
    return {"kind": "date", "iso": iso}


def time(iso: str) -> LoraTime:
    return {"kind": "time", "iso": iso}


def localtime(iso: str) -> LoraLocalTime:
    return {"kind": "localtime", "iso": iso}


def datetime(iso: str) -> LoraDateTime:
    return {"kind": "datetime", "iso": iso}


def localdatetime(iso: str) -> LoraLocalDateTime:
    return {"kind": "localdatetime", "iso": iso}


def duration(iso: str) -> LoraDuration:
    return {"kind": "duration", "iso": iso}


def vector(
    values: List[float],
    dimension: int,
    coordinate_type: LoraVectorCoordinateType,
) -> LoraVector:
    """Build a LoraVector param/value in the canonical tagged shape."""
    return {
        "kind": "vector",
        "dimension": dimension,
        "coordinateType": coordinate_type,
        "values": list(values),
    }


def cartesian(x: float, y: float) -> LoraCartesianPoint:
    return {"kind": "point", "srid": 7203, "crs": "cartesian", "x": x, "y": y}


def cartesian_3d(x: float, y: float, z: float) -> LoraCartesianPoint3D:
    return {
        "kind": "point",
        "srid": 9157,
        "crs": "cartesian-3D",
        "x": x,
        "y": y,
        "z": z,
    }


def wgs84(longitude: float, latitude: float) -> LoraWgs84Point:
    return {
        "kind": "point",
        "srid": 4326,
        "crs": "WGS-84-2D",
        "x": longitude,
        "y": latitude,
        "longitude": longitude,
        "latitude": latitude,
    }


def wgs84_3d(
    longitude: float, latitude: float, height: float
) -> LoraWgs84Point3D:
    return {
        "kind": "point",
        "srid": 4979,
        "crs": "WGS-84-3D",
        "x": longitude,
        "y": latitude,
        "z": height,
        "longitude": longitude,
        "latitude": latitude,
        "height": height,
    }


# ---------------------------------------------------------------------------
# Narrowing helpers.
# ---------------------------------------------------------------------------


def _is_tagged(v: Any, expected: str) -> bool:
    return isinstance(v, dict) and v.get("kind") == expected


def is_node(v: Any) -> bool:
    """Return True if ``v`` is a Lora node value."""
    return _is_tagged(v, "node")


def is_relationship(v: Any) -> bool:
    return _is_tagged(v, "relationship")


def is_path(v: Any) -> bool:
    return _is_tagged(v, "path")


def is_point(v: Any) -> bool:
    return _is_tagged(v, "point")


_TEMPORAL_KINDS = frozenset(
    {"date", "time", "localtime", "datetime", "localdatetime", "duration"}
)


def is_temporal(v: Any) -> bool:
    return isinstance(v, dict) and v.get("kind") in _TEMPORAL_KINDS


def is_vector(v: Any) -> bool:
    """Return True if ``v`` is a Lora VECTOR value."""
    return _is_tagged(v, "vector")
