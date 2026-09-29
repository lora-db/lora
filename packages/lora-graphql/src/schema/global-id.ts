import { fromBase64Url, toBase64Url } from "../compile/cursor.js";

/** Opaque global id: the type name and @key value. */
export function toGlobalId(type: string, key: unknown): string {
  return toBase64Url(
    JSON.stringify([type, typeof key === "bigint" ? key.toString() : key]),
  );
}

export function fromGlobalId(
  id: string,
): { type: string; key: string | number } | undefined {
  try {
    const decoded = JSON.parse(fromBase64Url(id)) as unknown;
    if (
      Array.isArray(decoded) &&
      decoded.length === 2 &&
      typeof decoded[0] === "string" &&
      (typeof decoded[1] === "string" || typeof decoded[1] === "number")
    ) {
      return { type: decoded[0], key: decoded[1] };
    }
  } catch {
    // fall through
  }
  return undefined;
}
