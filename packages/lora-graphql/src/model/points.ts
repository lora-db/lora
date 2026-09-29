import { requestError } from "../errors.js";

// GraphQL point inputs → the tagged point values LoraDB takes as
// parameters. WGS-84 inputs carry longitude/latitude (height for 3D);
// cartesian inputs carry x/y (z for 3D).

type PointInput = {
  longitude?: number;
  latitude?: number;
  height?: number | null;
  x?: number;
  y?: number;
  z?: number | null;
};

export function toLoraPoint(input: unknown): unknown {
  if (input === null || typeof input !== "object") return input;
  const p = input as PointInput;
  if (typeof p.longitude === "number" && typeof p.latitude === "number") {
    const base = {
      x: p.longitude,
      y: p.latitude,
      longitude: p.longitude,
      latitude: p.latitude,
    };
    return typeof p.height === "number"
      ? {
          kind: "point",
          srid: 4979,
          crs: "WGS-84-3D",
          ...base,
          z: p.height,
          height: p.height,
        }
      : { kind: "point", srid: 4326, crs: "WGS-84-2D", ...base };
  }
  if (typeof p.x === "number" && typeof p.y === "number") {
    return typeof p.z === "number"
      ? {
          kind: "point",
          srid: 9157,
          crs: "cartesian-3D",
          x: p.x,
          y: p.y,
          z: p.z,
        }
      : { kind: "point", srid: 7203, crs: "cartesian", x: p.x, y: p.y };
  }
  return input;
}

/**
 * A stored value as the engine takes it: points converted, lists mapped,
 * `@vector` fields tagged as VECTORs (a vector index skips plain lists).
 */
export function toStored(
  field: {
    name: string;
    type: string;
    vector?: { dimensions: number } | undefined;
  },
  value: unknown,
): unknown {
  if (field.vector && Array.isArray(value)) {
    if (value.length !== field.vector.dimensions) {
      throw requestError(
        "BAD_USER_INPUT",
        `${field.name} has ${value.length} dimensions; it needs ${field.vector.dimensions}`,
      );
    }
    return {
      kind: "vector",
      dimension: value.length,
      coordinateType: "FLOAT32",
      values: value,
    };
  }
  if (field.type !== "Point" && field.type !== "CartesianPoint") return value;
  return Array.isArray(value) ? value.map(toLoraPoint) : toLoraPoint(value);
}
