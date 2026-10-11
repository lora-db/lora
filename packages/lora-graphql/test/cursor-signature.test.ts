// A signed cursor carries the first 16 bytes of HMAC-SHA-256 over its
// payload. Node signs with node:crypto; the portable implementation
// (browsers) must give the same bytes, so a cursor signed by one build
// verifies in the other.

import { createHmac } from "node:crypto";
import { expect, test } from "vitest";
import { decodeCursor, encodeCursor } from "../src/compile/cursor.js";
import { hmacSha256 } from "../src/compile/hmac.js";

const base64url = (bytes: Uint8Array) =>
  Buffer.from(bytes).toString("base64url");

test("the signature is HMAC-SHA-256 of the payload, by either implementation", () => {
  for (const secret of ["s3cret", "k".repeat(100), "ключ"]) {
    const cursor = encodeCursor("name:ASC", ["Ada", 7], secret);
    const [payload, signature] = cursor.split(".");
    const native = createHmac("sha256", secret).update(payload!).digest();
    const portable = hmacSha256(
      new TextEncoder().encode(secret),
      new TextEncoder().encode(payload!),
    );
    expect(base64url(portable)).toBe(base64url(native));
    expect(signature).toBe(base64url(native.subarray(0, 16)));
    expect(decodeCursor(cursor, "name:ASC", 2, secret)).toEqual(["Ada", 7]);
  }
});

test("a cursor signed with another secret, or altered, is refused", () => {
  const cursor = encodeCursor("name:ASC", ["Ada"], "one");
  expect(() => decodeCursor(cursor, "name:ASC", 1, "two")).toThrow(
    "not a valid cursor",
  );
  const [payload, signature] = cursor.split(".");
  const forged = `${encodeCursor("name:ASC", ["Zed"]).split(".")[0]}.${signature}`;
  expect(() => decodeCursor(forged, "name:ASC", 1, "one")).toThrow(
    "not a valid cursor",
  );
  expect(decodeCursor(`${payload}.${signature}`, "name:ASC", 1, "one")).toEqual(
    ["Ada"],
  );
});
