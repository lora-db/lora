/**
 * Decoder for the binary result format produced by the native side.
 *
 * The native `execute()` ships a single `Uint8Array` whose layout is
 * defined by `crates/bindings/lora-binding-buffer/src/lib.rs`. This
 * file is the sole reader; it must stay byte-compatible with the
 * encoder there.
 *
 * Why this exists: each FFI syscall costs ~150 ns; for a 10 000-row
 * scan, the per-cell loop takes thousands of crossings and dominates
 * wall-clock time. Transferring the whole result as one buffer and
 * decoding it in JIT'd JavaScript is materially faster — V8 walks
 * contiguous bytes far more cheaply than the FFI layer can hand
 * values across one at a time.
 *
 * Shared between the `lora-node` and `lora-wasm` bindings; the source
 * of truth lives in `crates/bindings/shared-ts/decode.ts` and is
 * synced into each binding's `ts/` by their `npm run sync:types`
 * script (mirrors how `types.ts` is shared).
 */
/* eslint-disable @typescript-eslint/no-explicit-any */

const MAGIC_0 = 0x4c; // L
const MAGIC_1 = 0x52; // R
const MAGIC_2 = 0x31; // 1
const MAGIC_3 = 0x00;

// Cell tags — keep in sync with `tag::` constants in encode.rs.
const TAG_NULL = 0x00;
const TAG_FALSE = 0x01;
const TAG_TRUE = 0x02;
const TAG_I32 = 0x03;
const TAG_I64 = 0x04;
const TAG_F64 = 0x05;
const TAG_STRING = 0x06;
const TAG_LIST = 0x07;
const TAG_MAP = 0x08;
const TAG_NODE = 0x09;
const TAG_RELATIONSHIP = 0x0a;
const TAG_PATH = 0x0b;
const TAG_DATE = 0x0c;
const TAG_TIME = 0x0d;
const TAG_LOCAL_TIME = 0x0e;
const TAG_DATETIME = 0x0f;
const TAG_LOCAL_DATETIME = 0x10;
const TAG_DURATION = 0x11;
const TAG_POINT = 0x12;
const TAG_VECTOR = 0x13;
const TAG_BINARY = 0x14;

const VECTOR_FLOAT64 = 0;
const VECTOR_FLOAT32 = 1;
const VECTOR_INT64 = 2;
const VECTOR_INT32 = 3;
const VECTOR_INT16 = 4;
const VECTOR_INT8 = 5;

const SRID_CARTESIAN_2D = 7203;
const SRID_CARTESIAN_3D = 9157;
const SRID_WGS84_2D = 4326;
const SRID_WGS84_3D = 4979;

function sridToCrs(srid: number): string {
  switch (srid) {
    case SRID_CARTESIAN_2D:
      return "cartesian";
    case SRID_CARTESIAN_3D:
      return "cartesian-3D";
    case SRID_WGS84_2D:
      return "WGS-84-2D";
    case SRID_WGS84_3D:
      return "WGS-84-3D";
    default:
      return "cartesian";
  }
}

const utf8 = new TextDecoder("utf-8");

class Reader {
  readonly view: DataView;
  readonly bytes: Uint8Array;
  off = 0;

  constructor(buf: Uint8Array) {
    this.bytes = buf;
    this.view = new DataView(buf.buffer, buf.byteOffset, buf.byteLength);
  }

  u8(): number {
    // `noUncheckedIndexedAccess` says this might be undefined; we
    // never read past the buffer because the encoder writes a
    // bounded layout. The hot path can't afford a check.
    return this.bytes[this.off++]!;
  }

  u32(): number {
    const v = this.view.getUint32(this.off, true);
    this.off += 4;
    return v;
  }

  i32(): number {
    const v = this.view.getInt32(this.off, true);
    this.off += 4;
    return v;
  }

  /**
   * Reads an i64 LE and returns it as a JS Number. Values outside the
   * `Number.MAX_SAFE_INTEGER` range silently lose precision — same
   * behaviour the binding has always had on the read path. Callers that
   * need exact representation of large ints should use a separate
   * binding-side path; this is a hot loop.
   */
  i64AsNumber(): number {
    // Two getInt32 calls beat getBigInt64 + Number(BigInt) by ~3× in V8.
    // Manual reconstruction is exact for the |x| < 2^53 range.
    const lo = this.view.getUint32(this.off, true);
    const hi = this.view.getInt32(this.off + 4, true);
    this.off += 8;
    return hi * 0x1_0000_0000 + lo;
  }

  f64(): number {
    const v = this.view.getFloat64(this.off, true);
    this.off += 8;
    return v;
  }

  str(): string {
    const len = this.u32();
    const start = this.off;
    this.off += len;
    if (len === 0) return "";
    // TextDecoder is a fused C++ path in V8; faster than Buffer.toString
    // for short-to-medium strings and avoids hitting Buffer's own
    // string-cache layer.
    return utf8.decode(this.bytes.subarray(start, this.off));
  }
}

function readValue(r: Reader): any {
  const tag = r.u8();
  switch (tag) {
    case TAG_NULL:
      return null;
    case TAG_FALSE:
      return false;
    case TAG_TRUE:
      return true;
    case TAG_I32:
      return r.i32();
    case TAG_I64:
      return r.i64AsNumber();
    case TAG_F64:
      return r.f64();
    case TAG_STRING:
      return r.str();
    case TAG_LIST: {
      const n = r.u32();
      const arr = new Array(n);
      for (let i = 0; i < n; i++) arr[i] = readValue(r);
      return arr;
    }
    case TAG_MAP: {
      const n = r.u32();
      const obj: Record<string, any> = {};
      for (let i = 0; i < n; i++) {
        const k = r.str();
        obj[k] = readValue(r);
      }
      return obj;
    }
    case TAG_NODE:
      return {
        kind: "node",
        id: r.i64AsNumber(),
        labels: [],
        properties: {},
      };
    case TAG_RELATIONSHIP:
      return { kind: "relationship", id: r.i64AsNumber() };
    case TAG_PATH: {
      const nN = r.u32();
      const nodes = new Array<number>(nN);
      for (let i = 0; i < nN; i++) nodes[i] = r.i64AsNumber();
      const nR = r.u32();
      const rels = new Array<number>(nR);
      for (let i = 0; i < nR; i++) rels[i] = r.i64AsNumber();
      return { kind: "path", nodes, rels };
    }
    case TAG_DATE:
      return { kind: "date", iso: r.str() };
    case TAG_TIME:
      return { kind: "time", iso: r.str() };
    case TAG_LOCAL_TIME:
      return { kind: "localtime", iso: r.str() };
    case TAG_DATETIME:
      return { kind: "datetime", iso: r.str() };
    case TAG_LOCAL_DATETIME:
      return { kind: "localdatetime", iso: r.str() };
    case TAG_DURATION:
      return { kind: "duration", iso: r.str() };
    case TAG_POINT: {
      const hasZ = r.u8();
      const srid = r.u32();
      const x = r.f64();
      const y = r.f64();
      const point: Record<string, any> = {
        kind: "point",
        srid,
        crs: sridToCrs(srid),
        x,
        y,
      };
      if (hasZ) point.z = r.f64();
      if (srid === SRID_WGS84_2D || srid === SRID_WGS84_3D) {
        point.longitude = x;
        point.latitude = y;
        if (hasZ) point.height = point.z;
      }
      return point;
    }
    case TAG_VECTOR:
      return readVector(r);
    case TAG_BINARY:
      return readBinary(r);
    default:
      throw new Error(`unknown LoraValue tag: 0x${tag.toString(16)}`);
  }
}

function readVector(r: Reader): any {
  const coordTag = r.u8();
  const dim = r.u32();
  const values = new Array<number>(dim);
  let coordinateType: string;
  switch (coordTag) {
    case VECTOR_FLOAT64:
      coordinateType = "FLOAT64";
      for (let i = 0; i < dim; i++) values[i] = r.f64();
      break;
    case VECTOR_FLOAT32:
      coordinateType = "FLOAT32";
      for (let i = 0; i < dim; i++) {
        values[i] = r.view.getFloat32(r.off, true);
        r.off += 4;
      }
      break;
    case VECTOR_INT64:
      coordinateType = "INTEGER";
      for (let i = 0; i < dim; i++) values[i] = r.i64AsNumber();
      break;
    case VECTOR_INT32:
      coordinateType = "INTEGER32";
      for (let i = 0; i < dim; i++) values[i] = r.i32();
      break;
    case VECTOR_INT16:
      coordinateType = "INTEGER16";
      for (let i = 0; i < dim; i++) {
        values[i] = r.view.getInt16(r.off, true);
        r.off += 2;
      }
      break;
    case VECTOR_INT8:
      coordinateType = "INTEGER8";
      for (let i = 0; i < dim; i++) {
        values[i] = r.view.getInt8(r.off);
        r.off += 1;
      }
      break;
    default:
      throw new Error(`unknown vector coord type: ${coordTag}`);
  }
  return { kind: "vector", dimension: dim, coordinateType, values };
}

function readBinary(r: Reader): any {
  const segCount = r.u32();
  const segments = new Array<number[]>(segCount);
  let total = 0;
  for (let i = 0; i < segCount; i++) {
    const len = r.u32();
    const seg = new Array<number>(len);
    for (let j = 0; j < len; j++) seg[j] = r.bytes[r.off + j]!;
    r.off += len;
    segments[i] = seg;
    total += len;
  }
  return { kind: "binary", length: total, segments };
}

/**
 * Per-shape row factory. `new Function`-generated so the property
 * assignments inside the body are *static* — V8 sees a fixed object
 * literal shape and shares a hidden class across every row of the
 * same query. The dynamic `row[columns[j]] = …` form forces each row
 * to bootstrap its own hidden class, which is the dominant cost on
 * tall result sets after the napi-syscall overhead is removed.
 */
type RowFactory = (
  read: (r: Reader) => any,
  r: Reader,
) => Record<string, any>;

const factoryCache = new Map<string, RowFactory>();

function rowFactory(columns: readonly string[]): RowFactory {
  const key = columns.join("\x1f");
  let factory = factoryCache.get(key);
  if (factory) return factory;
  // Build:
  //   function (read, r) {
  //     const row = {};
  //     row[colA] = read(r);
  //     row[colB] = read(r);
  //     return row;
  //   }
  const lines: string[] = ["const row = {};"];
  for (const col of columns) {
    lines.push(`row[${JSON.stringify(col)}] = read(r);`);
  }
  lines.push("return row;");
  factory = new Function("read", "r", lines.join("\n")) as RowFactory;
  factoryCache.set(key, factory);
  return factory;
}

/**
 * Decode the wire format produced by `encode_rows` in the native crate
 * into the public `{ columns, rows }` shape.
 */
export function decodeResult(
  buf: Uint8Array,
): { columns: string[]; rows: Array<Record<string, any>> } {
  const r = new Reader(buf);
  if (
    r.bytes[0] !== MAGIC_0 ||
    r.bytes[1] !== MAGIC_1 ||
    r.bytes[2] !== MAGIC_2 ||
    r.bytes[3] !== MAGIC_3
  ) {
    throw new Error("lora-node: invalid result buffer magic");
  }
  r.off = 4;

  const colCount = r.u32();
  const columns = new Array<string>(colCount);
  for (let i = 0; i < colCount; i++) columns[i] = r.str();

  const rowCount = r.u32();
  const rows = new Array<Record<string, any>>(rowCount);
  if (colCount === 0) {
    for (let i = 0; i < rowCount; i++) rows[i] = {};
    return { columns, rows };
  }
  const factory = rowFactory(columns);
  for (let i = 0; i < rowCount; i++) {
    rows[i] = factory(readValue, r);
  }

  return { columns, rows };
}
