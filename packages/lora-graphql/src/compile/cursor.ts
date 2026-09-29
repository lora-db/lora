// Keyset cursors: the sort values of the last row plus the @key, signed
// with the sort they were produced under, so a cursor replayed against a
// different sort is rejected instead of silently skipping rows.

import { requestError } from "../errors.js";

interface CursorPayload {
  s: string;
  v: unknown[];
}

export function encodeCursor(sort: string, values: unknown[]): string {
  const json = JSON.stringify(
    { s: sort, v: values } satisfies CursorPayload,
    (_k, val) => (typeof val === "bigint" ? { $bigint: val.toString() } : val),
  );
  return toBase64Url(json);
}

export function decodeCursor(
  cursor: string,
  sort: string,
  arity: number,
): unknown[] {
  let payload: CursorPayload;
  try {
    payload = JSON.parse(fromBase64Url(cursor), (_k, val) =>
      val !== null &&
      typeof val === "object" &&
      typeof (val as { $bigint?: unknown }).$bigint === "string"
        ? BigInt((val as { $bigint: string }).$bigint)
        : val,
    ) as CursorPayload;
  } catch {
    throw requestError("INVALID_CURSOR", "`after` is not a valid cursor");
  }
  if (
    typeof payload !== "object" ||
    payload === null ||
    !Array.isArray(payload.v) ||
    payload.v.length !== arity
  ) {
    throw requestError("INVALID_CURSOR", "`after` is not a valid cursor");
  }
  if (payload.s !== sort) {
    throw requestError(
      "INVALID_CURSOR",
      "`after` was produced under a different sort; request the first page again",
    );
  }
  return payload.v;
}

// Portable base64url (Node and browsers; the WASM binding runs in both).
export function toBase64Url(text: string): string {
  let bin = "";
  for (const b of new TextEncoder().encode(text)) bin += String.fromCharCode(b);
  return btoa(bin).replace(/\+/g, "-").replace(/\//g, "_").replace(/=+$/, "");
}

export function fromBase64Url(encoded: string): string {
  const b64 = encoded.replace(/-/g, "+").replace(/_/g, "/");
  const bin = atob(b64 + "=".repeat((4 - (b64.length % 4)) % 4));
  return new TextDecoder("utf-8", { fatal: true }).decode(
    Uint8Array.from(bin, (c) => c.charCodeAt(0)),
  );
}
