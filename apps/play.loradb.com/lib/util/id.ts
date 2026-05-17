/**
 * Tiny dependency-free ULID generator.
 *
 * Produces a 26-character Crockford base32 string: 10 chars of timestamp
 * (millisecond resolution, ~10889 AD overflow) followed by 16 chars of
 * randomness. Uses `crypto.getRandomValues` when available and falls back
 * to `Math.random` otherwise.
 *
 * This is deterministic in shape (always 26 chars, always sortable by
 * generation time) but not cryptographically rigorous — it is intended
 * for client-side run/correlation IDs, not security tokens.
 */

const ENCODING = "0123456789ABCDEFGHJKMNPQRSTVWXYZ"; // Crockford base32
const ENCODING_LEN = ENCODING.length;
const TIME_LEN = 10;
const RANDOM_LEN = 16;

function randomBytes(n: number): Uint8Array {
  const buf = new Uint8Array(n);
  const g: { crypto?: { getRandomValues?: (b: Uint8Array) => Uint8Array } } =
    typeof globalThis === "undefined" ? {} : (globalThis as unknown as typeof g);
  if (g.crypto && typeof g.crypto.getRandomValues === "function") {
    g.crypto.getRandomValues(buf);
    return buf;
  }
  for (let i = 0; i < n; i++) {
    buf[i] = Math.floor(Math.random() * 256);
  }
  return buf;
}

function encodeTime(now: number, len: number): string {
  let mod: number;
  let out = "";
  let t = now;
  for (let i = len - 1; i >= 0; i--) {
    mod = t % ENCODING_LEN;
    out = ENCODING.charAt(mod) + out;
    t = (t - mod) / ENCODING_LEN;
  }
  return out;
}

function encodeRandom(len: number): string {
  // 16 chars * 5 bits = 80 bits. Pull a generous byte budget and map per char.
  const bytes = randomBytes(len);
  let out = "";
  for (let i = 0; i < len; i++) {
    const byte = bytes[i] ?? 0;
    out += ENCODING.charAt(byte % ENCODING_LEN);
  }
  return out;
}

/** Returns a 26-character Crockford-base32 ULID string. */
export function ulid(): string {
  return encodeTime(Date.now(), TIME_LEN) + encodeRandom(RANDOM_LEN);
}
