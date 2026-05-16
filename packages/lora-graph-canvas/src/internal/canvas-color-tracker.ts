// Indexed object → unique-color registry for shadow-canvas hit
// testing. Each registered object gets a deterministic RGB encoded
// from its registry index plus a checksum (so a random pixel rarely
// hashes to a valid registry entry). `lookup` reverses an [r,g,b]
// triple back to its object, or returns null if the checksum fails
// or the index is out of bounds.
//
// LORA: internalised from `canvas-color-tracker` (MIT, © Vasco
// Asturiano). The upstream parses arbitrary colour strings via
// `tinycolor2`; we only ever store and round-trip the hex strings
// our `register()` returns (#RRGGBB), so we replace the dep with a
// tiny 6-char hex parser. Pixel hits arrive as [r,g,b] from
// `ImageData.data` and skip the parser entirely.

const ENTROPY = 123; // Bumps low-index numbers above the noise floor.

const int2HexColor = (num: number): string =>
  `#${Math.min(num, 2 ** 24).toString(16).padStart(6, "0")}`;

const rgb2Int = (r: number, g: number, b: number): number =>
  (r << 16) + (g << 8) + b;

const checksum = (n: number, csBits: number): number =>
  (n * ENTROPY) % 2 ** csBits;

export default class CanvasColorTracker<T = unknown> {
  /** How many of the 24 bits are reserved for the checksum. The
   *  remainder bounds the registry's max addressable size. */
  readonly #csBits: number;
  #registry!: Array<T | "__reserved for background__">;

  constructor(csBits: number = 6) {
    this.#csBits = csBits;
    this.reset();
  }

  reset(): void {
    this.#registry = ["__reserved for background__"];
  }

  /** Allocate a fresh colour for `obj` and return its hex string,
   *  or `null` once the registry is full. */
  register(obj: T): string | null {
    const cap = 2 ** (24 - this.#csBits);
    if (this.#registry.length >= cap) return null;
    const idx = this.#registry.length;
    const cs = checksum(idx, this.#csBits);
    const color = int2HexColor(idx + (cs << (24 - this.#csBits)));
    this.#registry.push(obj);
    return color;
  }

  /** Reverse a hex string (as returned by `register`) or an [r,g,b]
   *  triple (as read from `ImageData.data`) back to its registered
   *  object. Returns `null` if the colour doesn't decode to a valid
   *  entry or fails the checksum. */
  lookup(color: string | [number, number, number]): T | null {
    if (!color) return null;
    const n =
      typeof color === "string"
        ? hexStr2Int(color)
        : rgb2Int(color[0], color[1], color[2]);
    if (!n) return null; // 0 is the background sentinel
    const idx = n & ((2 ** (24 - this.#csBits)) - 1);
    const cs = (n >> (24 - this.#csBits)) & ((2 ** this.#csBits) - 1);
    if (
      checksum(idx, this.#csBits) !== cs ||
      idx >= this.#registry.length
    ) {
      return null;
    }
    const entry = this.#registry[idx];
    return entry === "__reserved for background__" ? null : (entry as T);
  }
}

/** Parse a `#RRGGBB` hex colour into a 24-bit int. Returns 0 for
 *  malformed input — the tracker treats 0 as the background sentinel,
 *  so a parse failure naturally maps to a no-hit. */
function hexStr2Int(s: string): number {
  if (!s || s[0] !== "#" || s.length !== 7) return 0;
  const n = parseInt(s.slice(1), 16);
  return Number.isNaN(n) ? 0 : n;
}
