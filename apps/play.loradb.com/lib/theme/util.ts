/**
 * Tiny colour helpers used by the theme derivers. Kept dependency-free
 * so they're trivial to unit-test and safe to import from anywhere.
 */

import type { MantineColorsTuple } from "@mantine/core";

const HEX_RE = /^#([0-9a-fA-F]{6})$/;

interface Rgb {
  r: number;
  g: number;
  b: number;
}

interface Hsl {
  h: number;
  s: number;
  l: number;
}

function parseHex(hex: string): Rgb {
  const m = HEX_RE.exec(hex);
  if (!m) {
    throw new Error(
      `hexA: expected a #rrggbb colour, got ${JSON.stringify(hex)}`,
    );
  }
  const body = m[1] as string;
  return {
    r: parseInt(body.slice(0, 2), 16),
    g: parseInt(body.slice(2, 4), 16),
    b: parseInt(body.slice(4, 6), 16),
  };
}

/**
 * Compose `#rrggbb` + an alpha in [0, 1] into a `rgba(r, g, b, a)` string.
 * Throws on malformed hex or out-of-range alpha.
 */
export function hexA(hex: string, alpha: number): string {
  if (!Number.isFinite(alpha) || alpha < 0 || alpha > 1) {
    throw new Error(
      `hexA: alpha must be a finite number in [0, 1], got ${alpha}`,
    );
  }
  const { r, g, b } = parseHex(hex);
  const a = Math.round(alpha * 1000) / 1000;
  return `rgba(${r}, ${g}, ${b}, ${a})`;
}

function rgbToHsl({ r, g, b }: Rgb): Hsl {
  const rn = r / 255;
  const gn = g / 255;
  const bn = b / 255;
  const max = Math.max(rn, gn, bn);
  const min = Math.min(rn, gn, bn);
  const l = (max + min) / 2;
  let h = 0;
  let s = 0;
  if (max !== min) {
    const d = max - min;
    s = l > 0.5 ? d / (2 - max - min) : d / (max + min);
    switch (max) {
      case rn:
        h = (gn - bn) / d + (gn < bn ? 6 : 0);
        break;
      case gn:
        h = (bn - rn) / d + 2;
        break;
      default:
        h = (rn - gn) / d + 4;
        break;
    }
    h /= 6;
  }
  return { h, s, l };
}

function hue2rgb(p: number, q: number, t: number): number {
  let tt = t;
  if (tt < 0) tt += 1;
  if (tt > 1) tt -= 1;
  if (tt < 1 / 6) return p + (q - p) * 6 * tt;
  if (tt < 1 / 2) return q;
  if (tt < 2 / 3) return p + (q - p) * (2 / 3 - tt) * 6;
  return p;
}

function hslToRgb({ h, s, l }: Hsl): Rgb {
  let r: number;
  let g: number;
  let b: number;
  if (s === 0) {
    r = g = b = l;
  } else {
    const q = l < 0.5 ? l * (1 + s) : l + s - l * s;
    const p = 2 * l - q;
    r = hue2rgb(p, q, h + 1 / 3);
    g = hue2rgb(p, q, h);
    b = hue2rgb(p, q, h - 1 / 3);
  }
  return {
    r: Math.round(r * 255),
    g: Math.round(g * 255),
    b: Math.round(b * 255),
  };
}

function toHex({ r, g, b }: Rgb): string {
  const h = (n: number) =>
    Math.max(0, Math.min(255, n)).toString(16).padStart(2, "0");
  return `#${h(r)}${h(g)}${h(b)}`;
}

/**
 * Generate a Mantine 10-shade tuple by varying HSL lightness around
 * the base hex. Shades go from light (index 0) to dark (index 9); the
 * base hex anchors index 6, which is Mantine's conventional "primary"
 * slot for `primaryShade`.
 *
 * The mapping is intentionally simple — we're not trying to match
 * Mantine's curated tuples 1:1, just produce a usable ramp so
 * `<Button color="brand">` and friends look coherent.
 */
export function shadesFrom(hex: string): MantineColorsTuple {
  const base = rgbToHsl(parseHex(hex));
  // Lightness targets for indices 0..9 (light → dark).
  const targets = [0.95, 0.88, 0.78, 0.68, 0.58, 0.5, base.l, 0.34, 0.24, 0.16];
  const shades = targets.map((l) =>
    toHex(
      hslToRgb({ h: base.h, s: base.s, l: Math.max(0.05, Math.min(0.97, l)) }),
    ),
  );
  return [
    shades[0] as string,
    shades[1] as string,
    shades[2] as string,
    shades[3] as string,
    shades[4] as string,
    shades[5] as string,
    shades[6] as string,
    shades[7] as string,
    shades[8] as string,
    shades[9] as string,
  ];
}
