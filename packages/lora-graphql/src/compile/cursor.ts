// Keyset cursors: the sort values of the last row plus the @key, tagged
// with the sort they were produced under, so a cursor replayed against a
// different sort is rejected instead of silently skipping rows. With a
// `cursorSecret` they are also signed (HMAC-SHA-256), so a client cannot
// forge one; without it they are only tagged.

import { requestError } from "../errors.js";
import { constantTimeEqual, hmacSha256 } from "./hmac.js";

interface CursorPayload {
  s: string;
  v: unknown[];
}

/** Bytes of the signature kept in a signed cursor. */
const SIGNATURE_BYTES = 16;

export function encodeCursor(
  sort: string,
  values: unknown[],
  secret?: string,
): string {
  const json = JSON.stringify(
    { s: sort, v: values } satisfies CursorPayload,
    (_k, val) => (typeof val === "bigint" ? { $bigint: val.toString() } : val),
  );
  const payload = toBase64Url(json);
  return secret === undefined
    ? payload
    : `${payload}.${bytesToBase64Url(sign(secret, payload))}`;
}

export function decodeCursor(
  cursor: string,
  sort: string,
  arity: number,
  secret?: string,
): unknown[] {
  let encoded = cursor;
  if (secret !== undefined) {
    const dot = cursor.indexOf(".");
    let given: Uint8Array | undefined;
    try {
      given = dot > 0 ? base64UrlToBytes(cursor.slice(dot + 1)) : undefined;
    } catch {
      given = undefined;
    }
    encoded = cursor.slice(0, Math.max(dot, 0));
    if (!given || !constantTimeEqual(given, sign(secret, encoded))) {
      throw requestError("INVALID_CURSOR", "`after` is not a valid cursor");
    }
  }
  let payload: CursorPayload;
  try {
    payload = JSON.parse(fromBase64Url(encoded), (_k, val) =>
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

function sign(secret: string, payload: string): Uint8Array {
  const encoder = new TextEncoder();
  return hmacSha256(encoder.encode(secret), encoder.encode(payload)).slice(
    0,
    SIGNATURE_BYTES,
  );
}

// Portable base64url (Node and browsers; the WASM binding runs in both).
export function toBase64Url(text: string): string {
  return bytesToBase64Url(new TextEncoder().encode(text));
}

export function fromBase64Url(encoded: string): string {
  return new TextDecoder("utf-8", { fatal: true }).decode(
    base64UrlToBytes(encoded),
  );
}

function bytesToBase64Url(bytes: Uint8Array): string {
  let bin = "";
  for (const b of bytes) bin += String.fromCharCode(b);
  return btoa(bin).replace(/\+/g, "-").replace(/\//g, "_").replace(/=+$/, "");
}

function base64UrlToBytes(encoded: string): Uint8Array {
  const b64 = encoded.replace(/-/g, "+").replace(/_/g, "/");
  const bin = atob(b64 + "=".repeat((4 - (b64.length % 4)) % 4));
  return Uint8Array.from(bin, (c) => c.charCodeAt(0));
}
