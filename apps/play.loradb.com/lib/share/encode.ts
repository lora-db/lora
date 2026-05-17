/**
 * Share-URL encoding helpers.
 *
 * Encoded as `#q=<lz-string compressed>` so the query body survives copy/paste
 * across browsers and chat clients without %-encoding chains. The decode side
 * lives in `./decode.ts`.
 */

import LZString from "lz-string";

/** Compresses `body` into a URL-component-safe string for the `#q=` hash. */
export function encodeQuery(body: string): string {
  return LZString.compressToEncodedURIComponent(body);
}

/** Returns a full `"#q=<encoded>"` hash fragment for the given query body. */
export function makeShareHash(body: string): string {
  return `#q=${encodeQuery(body)}`;
}
