/**
 * Share-URL decoding helpers — the inverse of `./encode.ts`.
 *
 * `decodeQuery` returns `null` for any malformed input rather than
 * throwing, because share links are user-supplied data and we never
 * want a bad paste to crash the workbench.
 */

import LZString from "lz-string";

/** Decompresses a share-encoded query. Returns `null` for invalid input. */
export function decodeQuery(value: string): string | null {
  if (typeof value !== "string" || value.length === 0) return null;
  try {
    const result = LZString.decompressFromEncodedURIComponent(value);
    // lz-string returns "" for invalid input on some versions and null on others.
    if (result === null || result === "") return null;
    return result;
  } catch {
    return null;
  }
}

/**
 * Decompresses a share-encoded params payload. Returns `null` for
 * invalid input; the caller falls back to the slice's `"{}"` default.
 */
export function decodeParams(value: string): string | null {
  if (typeof value !== "string" || value.length === 0) return null;
  try {
    const result = LZString.decompressFromEncodedURIComponent(value);
    if (result === null || result === "") return null;
    return result;
  } catch {
    return null;
  }
}

/** Parses a `"#q=...&snap=...&p=..."`-style hash fragment into its known fields. */
export function readHash(hash: string): {
  q?: string;
  snap?: string;
  p?: string;
} {
  if (typeof hash !== "string" || hash.length === 0) return {};
  // Tolerate both `#a=b&c=d` and `a=b&c=d`.
  const trimmed = hash.startsWith("#") ? hash.slice(1) : hash;
  if (trimmed.length === 0) return {};

  const out: { q?: string; snap?: string; p?: string } = {};
  for (const part of trimmed.split("&")) {
    if (part.length === 0) continue;
    const eq = part.indexOf("=");
    const key = eq >= 0 ? part.slice(0, eq) : part;
    const value = eq >= 0 ? part.slice(eq + 1) : "";
    if (key === "q") {
      out.q = value;
    } else if (key === "snap") {
      out.snap = value;
    } else if (key === "p") {
      out.p = value;
    }
  }
  return out;
}
