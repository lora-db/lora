/**
 * Share-URL encoding helpers.
 *
 * Encoded as `#q=<lz-string compressed>` so the query body survives
 * copy/paste across browsers and chat clients without %-encoding
 * chains. An optional `&p=<lz-string compressed>` carries the raw
 * JSON params payload alongside, so parameterised queries can be
 * shared as runnable templates.
 *
 * The decode side lives in `./decode.ts`.
 */

import LZString from "lz-string";

/** Compresses `body` into a URL-component-safe string for the `#q=` hash. */
export function encodeQuery(body: string): string {
  return LZString.compressToEncodedURIComponent(body);
}

/**
 * Compresses the raw JSON `$param` payload for the `&p=` hash
 * fragment. Symmetric counterpart to {@link encodeQuery}.
 */
export function encodeParams(params: string): string {
  return LZString.compressToEncodedURIComponent(params);
}

/**
 * Returns a full `"#q=<encoded>[&p=<encoded>]"` hash fragment for
 * the given query body, optionally with a params payload. The empty
 * payload (`undefined`, `""`, or `"{}"`) is omitted so trivial cases
 * keep their compact `#q=…` shape — saves URL characters where it
 * matters.
 */
export function makeShareHash(body: string, params?: string): string {
  const head = `#q=${encodeQuery(body)}`;
  if (params === undefined) return head;
  const trimmed = params.trim();
  if (trimmed === "" || trimmed === "{}") return head;
  return `${head}&p=${encodeParams(params)}`;
}
