/**
 * Canonical typed value model for the JS-facing Lora packages
 * (`lora-node` and `lora-wasm`).
 *
 * Every structural value carries a `kind` discriminator. Temporal and
 * spatial values are tagged so TypeScript can narrow safely after a
 * query runs.
 *
 * Canonical source: `crates/shared-ts/types.ts`. Each JS-facing package
 * (`lora-node`, `lora-wasm`) copies this file into its own `ts/` dir
 * via its `sync:types` npm script. The `verify:types` script fails CI if
 * the copies drift — do not edit the per-package copies directly.
 */

// ---------------------------------------------------------------------------
// Params (input)
// ---------------------------------------------------------------------------

/**
 * Values accepted as Lora query parameters. Mirrors `LoraValue` but
 * also allows plain JS scalars / arrays / objects. Temporal and spatial
 * values must be passed as tagged objects so the bridge can parse them
 * without ambiguity.
 */
export type LoraParam =
  | null
  | boolean
  | number
  | string
  | LoraParam[]
  | { [key: string]: LoraParam }
  | LoraDate
  | LoraTime
  | LoraLocalTime
  | LoraDateTime
  | LoraLocalDateTime
  | LoraDuration
  | LoraPoint
  | LoraVector
  | LoraBinary;

export type LoraParams = Record<string, LoraParam>;

// ---------------------------------------------------------------------------
// Structural return types
// ---------------------------------------------------------------------------

export interface LoraNode {
  kind: "node";
  id: number;
  labels: string[];
  properties: Record<string, LoraValue>;
}

export interface LoraRelationship {
  kind: "relationship";
  id: number;
  startId: number;
  endId: number;
  type: string;
  properties: Record<string, LoraValue>;
}

export interface LoraPath {
  kind: "path";
  nodes: number[];
  rels: number[];
}

// ---------------------------------------------------------------------------
// Temporal — ISO-8601 string, tagged
// ---------------------------------------------------------------------------

export interface LoraDate {
  kind: "date";
  /** `YYYY-MM-DD` */
  iso: string;
}

export interface LoraTime {
  kind: "time";
  /** `HH:MM:SS[.nnn][Z|±HH:MM]` */
  iso: string;
}

export interface LoraLocalTime {
  kind: "localtime";
  /** `HH:MM:SS[.nnn]` */
  iso: string;
}

export interface LoraDateTime {
  kind: "datetime";
  /** `YYYY-MM-DDTHH:MM:SS[.nnn][Z|±HH:MM]` */
  iso: string;
}

export interface LoraLocalDateTime {
  kind: "localdatetime";
  /** `YYYY-MM-DDTHH:MM:SS[.nnn]` */
  iso: string;
}

export interface LoraDuration {
  kind: "duration";
  /** ISO-8601 duration, e.g. `P1Y2M3DT4H5M6S` */
  iso: string;
}

export type TemporalKind =
  | "date"
  | "time"
  | "localtime"
  | "datetime"
  | "localdatetime"
  | "duration";

export type LoraTemporal =
  | LoraDate
  | LoraTime
  | LoraLocalTime
  | LoraDateTime
  | LoraLocalDateTime
  | LoraDuration;

// ---------------------------------------------------------------------------
// Spatial
// ---------------------------------------------------------------------------

/**
 * Supported spatial-reference identifiers.
 *
 * - `7203` — Cartesian 2D
 * - `9157` — Cartesian 3D
 * - `4326` — WGS-84 Geographic 2D
 * - `4979` — WGS-84 Geographic 3D
 */
export type LoraPointSrid = 7203 | 9157 | 4326 | 4979;

/**
 * Canonical CRS name string. Mirrors `srid` 1:1 on the output side; on
 * the input side, `point({…, crs: "…"})` accepts these plus the alias
 * `"WGS-84"` (equivalent to `"WGS-84-2D"`) — see the engine README.
 */
export type LoraPointCrs =
  | "cartesian"
  | "cartesian-3D"
  | "WGS-84-2D"
  | "WGS-84-3D";

/** Cartesian 2D point. */
export interface LoraCartesianPoint {
  kind: "point";
  srid: 7203;
  crs: "cartesian";
  x: number;
  y: number;
}

/** Cartesian 3D point. */
export interface LoraCartesianPoint3D {
  kind: "point";
  srid: 9157;
  crs: "cartesian-3D";
  x: number;
  y: number;
  z: number;
}

/**
 * WGS-84 geographic 2D point.
 *
 * `x === longitude` and `y === latitude`. Both naming conventions are
 * exposed so consumers can pick whichever is clearer at the call site.
 */
export interface LoraWgs84Point {
  kind: "point";
  srid: 4326;
  crs: "WGS-84-2D";
  x: number;
  y: number;
  longitude: number;
  latitude: number;
}

/**
 * WGS-84 geographic 3D point.
 *
 * `x === longitude`, `y === latitude`, `z === height` (metres).
 *
 * **Caveat:** `distance()` on WGS-84-3D points currently ignores
 * `height` and computes the great-circle surface distance only. A full
 * 3D geodesic distance is not implemented.
 */
export interface LoraWgs84Point3D {
  kind: "point";
  srid: 4979;
  crs: "WGS-84-3D";
  x: number;
  y: number;
  z: number;
  longitude: number;
  latitude: number;
  height: number;
}

/**
 * Any point value returned by the engine. Narrow via the `srid` or
 * `crs` discriminator (or the `isPoint` guard plus `point.srid`).
 */
export type LoraPoint =
  | LoraCartesianPoint
  | LoraCartesianPoint3D
  | LoraWgs84Point
  | LoraWgs84Point3D;

// ---------------------------------------------------------------------------
// Vector
// ---------------------------------------------------------------------------

/**
 * Canonical coordinate type emitted for every VECTOR value returned by
 * the engine. Aliases (`FLOAT`, `INT`, `SIGNED INTEGER`, …) are accepted
 * by the `vector()` constructor on input but normalised to one of these
 * six tags on the wire.
 */
export type LoraVectorCoordinateType =
  | "FLOAT64"
  | "FLOAT32"
  | "INTEGER"
  | "INTEGER32"
  | "INTEGER16"
  | "INTEGER8";

/**
 * Tagged VECTOR value.
 *
 * `values.length` always equals `dimension`. Values are rendered as JS
 * numbers regardless of the underlying coordinate type — precision for
 * small-integer vectors is preserved because INTEGER* types always fit
 * in an `f64` mantissa.
 */
export interface LoraVector {
  kind: "vector";
  dimension: number;
  coordinateType: LoraVectorCoordinateType;
  values: number[];
}

// ---------------------------------------------------------------------------
// Binary
// ---------------------------------------------------------------------------

/**
 * Segmented binary/blob value. Each segment is a byte array; concatenate
 * segments in order to reconstruct the logical binary value.
 */
export interface LoraBinary {
  kind: "binary";
  length: number;
  segments: number[][];
}

// ---------------------------------------------------------------------------
// Value union
// ---------------------------------------------------------------------------

export type LoraValue =
  | null
  | boolean
  | number
  | string
  | LoraValue[]
  | { [key: string]: LoraValue }
  | LoraNode
  | LoraRelationship
  | LoraPath
  | LoraDate
  | LoraTime
  | LoraLocalTime
  | LoraDateTime
  | LoraLocalDateTime
  | LoraDuration
  | LoraPoint
  | LoraVector
  | LoraBinary;

export type QueryRow<T extends Record<string, LoraValue> = Record<string, LoraValue>> = T;

export interface QueryResult<
  T extends Record<string, LoraValue> = Record<string, LoraValue>,
> {
  columns: string[];
  rows: QueryRow<T>[];
}

// ---------------------------------------------------------------------------
// Guards
// ---------------------------------------------------------------------------

export function isNode(v: LoraValue): v is LoraNode {
  return typeof v === "object" && v !== null && !Array.isArray(v) && (v as { kind?: unknown }).kind === "node";
}

export function isRelationship(v: LoraValue): v is LoraRelationship {
  return typeof v === "object" && v !== null && !Array.isArray(v) && (v as { kind?: unknown }).kind === "relationship";
}

export function isPath(v: LoraValue): v is LoraPath {
  return typeof v === "object" && v !== null && !Array.isArray(v) && (v as { kind?: unknown }).kind === "path";
}

export function isPoint(v: LoraValue): v is LoraPoint {
  return typeof v === "object" && v !== null && !Array.isArray(v) && (v as { kind?: unknown }).kind === "point";
}

export function isVector(v: LoraValue): v is LoraVector {
  return typeof v === "object" && v !== null && !Array.isArray(v) && (v as { kind?: unknown }).kind === "vector";
}

export function isBinary(v: LoraValue): v is LoraBinary {
  return typeof v === "object" && v !== null && !Array.isArray(v) && (v as { kind?: unknown }).kind === "binary";
}

export function isTemporal(v: LoraValue): v is LoraTemporal {
  if (typeof v !== "object" || v === null || Array.isArray(v)) return false;
  const kind = (v as { kind?: unknown }).kind;
  return (
    kind === "date" ||
    kind === "time" ||
    kind === "localtime" ||
    kind === "datetime" ||
    kind === "localdatetime" ||
    kind === "duration"
  );
}

// ---------------------------------------------------------------------------
// Constructors
// ---------------------------------------------------------------------------

export const date = (iso: string): LoraDate => ({ kind: "date", iso });
export const time = (iso: string): LoraTime => ({ kind: "time", iso });
export const localtime = (iso: string): LoraLocalTime => ({ kind: "localtime", iso });
export const datetime = (iso: string): LoraDateTime => ({ kind: "datetime", iso });
export const localdatetime = (iso: string): LoraLocalDateTime => ({ kind: "localdatetime", iso });
export const duration = (iso: string): LoraDuration => ({ kind: "duration", iso });

/**
 * Build a `LoraVector` parameter value. Mirrors the on-wire tagged
 * shape the engine emits, so round-trips (receive from a query, pass
 * back into the next query) work without any conversion.
 */
export const vector = (
  values: number[],
  dimension: number,
  coordinateType: LoraVectorCoordinateType,
): LoraVector => ({ kind: "vector", dimension, coordinateType, values });

export const binary = (segments: number[][]): LoraBinary => ({
  kind: "binary",
  length: segments.reduce((sum, segment) => sum + segment.length, 0),
  segments,
});

export const cartesian = (x: number, y: number): LoraCartesianPoint => ({
  kind: "point",
  srid: 7203,
  crs: "cartesian",
  x,
  y,
});

export const cartesian3d = (
  x: number,
  y: number,
  z: number,
): LoraCartesianPoint3D => ({
  kind: "point",
  srid: 9157,
  crs: "cartesian-3D",
  x,
  y,
  z,
});

export const wgs84 = (
  longitude: number,
  latitude: number,
): LoraWgs84Point => ({
  kind: "point",
  srid: 4326,
  crs: "WGS-84-2D",
  x: longitude,
  y: latitude,
  longitude,
  latitude,
});

export const wgs84_3d = (
  longitude: number,
  latitude: number,
  height: number,
): LoraWgs84Point3D => ({
  kind: "point",
  srid: 4979,
  crs: "WGS-84-3D",
  x: longitude,
  y: latitude,
  z: height,
  longitude,
  latitude,
  height,
});

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

/**
 * Error codes emitted by the engine bridges.
 *
 * - `LORA_ERROR` — parse / analyze / execute failure
 * - `INVALID_PARAMS` — a param value could not be mapped to a Lora value
 * - `WORKER_ERROR` — worker transport / lifecycle failure (wasm worker only)
 * - `UNKNOWN` — fall-through for unrecognised error shapes
 */
export type LoraErrorCode =
  | "LORA_ERROR"
  | "INVALID_PARAMS"
  | "WORKER_ERROR"
  | "UNKNOWN";

/**
 * Error thrown by `Database.execute` when Lora parsing, analysis, or
 * execution fails.
 */
export class LoraError extends Error {
  public readonly code: LoraErrorCode;

  constructor(message: string, code: LoraErrorCode = "UNKNOWN") {
    super(message);
    this.name = "LoraError";
    this.code = code;
  }
}

const ERROR_PREFIX_RE = /^(LORA_ERROR|INVALID_PARAMS|WORKER_ERROR):\s*(.*)$/s;

/**
 * Normalise a thrown value into a `LoraError` with a narrowed `code`
 * when the message carries a recognised prefix; otherwise returns the
 * original `Error` unchanged.
 */
export function wrapError(err: unknown): Error {
  if (!(err instanceof Error)) return new LoraError(String(err), "UNKNOWN");
  const match = ERROR_PREFIX_RE.exec(err.message);
  if (match) {
    return new LoraError(match[2]!, match[1] as LoraErrorCode);
  }
  return err;
}
