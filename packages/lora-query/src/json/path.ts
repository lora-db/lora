/**
 * Synchronous JSON-path walker. Given a source string and a byte
 * offset, return the path segments leading to that offset.
 *
 * Examples:
 *   getJsonPath('{ "a": { "b": [1, 2] } }', 18) → ["a", "b", 0]
 *   getJsonPath('[ {"name": "Alice"} ]',    14) → [0, "name"]
 *
 * Tolerant of unfinished input — never throws, returns the deepest
 * path it could resolve. Used by the editor to render a breadcrumb
 * of the cursor's location.
 */

export type PathSegment = string | number;

/**
 * Resolve the JSON path to the cursor at `pos`. Returns an empty
 * array when the cursor is at the document root or the buffer
 * does not start with `{` / `[`.
 */
export function getJsonPath(source: string, pos: number): PathSegment[] {
  const path: PathSegment[] = [];
  let i = 0;
  const len = source.length;
  const target = Math.min(Math.max(pos, 0), len);

  while (i < target) {
    const ch = source[i];

    // Strings — including object keys. If the closing `"` is past
    // `target`, the cursor is inside that string, so we leave
    // without descending.
    if (ch === '"') {
      const close = findStringEnd(source, i);
      if (close === -1 || close >= target) return path;
      i = close + 1;
      continue;
    }

    // Whitespace.
    if (ch === " " || ch === "\t" || ch === "\n" || ch === "\r") {
      i++;
      continue;
    }

    if (ch === "{") {
      // Descend into object: find the first key, push it, advance
      // past the key + colon.
      i++;
      i = skipWhitespace(source, i);
      if (source[i] === "}") {
        // Empty object — pop later if we ever advance past.
        path.push("");
        if (i >= target) return path;
        i++;
        path.pop();
        continue;
      }
      const keyEnd = readKey(source, i);
      if (keyEnd === null) return path;
      path.push(keyEnd.key);
      i = keyEnd.end;
      // Skip past `:` if we haven't blown past the target.
      i = skipWhitespace(source, i);
      if (source[i] === ":") i++;
      i = skipWhitespace(source, i);
      continue;
    }

    if (ch === "[") {
      i++;
      i = skipWhitespace(source, i);
      if (source[i] === "]") {
        path.push(0);
        if (i >= target) return path;
        i++;
        path.pop();
        continue;
      }
      path.push(0);
      continue;
    }

    if (ch === ",") {
      // Bump the trailing path segment: next array index, or next
      // object key.
      i++;
      i = skipWhitespace(source, i);
      const top = path[path.length - 1];
      if (typeof top === "number") {
        path[path.length - 1] = top + 1;
      } else if (typeof top === "string") {
        // Read the next key from the comma onwards.
        const keyEnd = readKey(source, i);
        if (keyEnd) {
          path[path.length - 1] = keyEnd.key;
          i = keyEnd.end;
          i = skipWhitespace(source, i);
          if (source[i] === ":") i++;
          i = skipWhitespace(source, i);
        }
      }
      continue;
    }

    if (ch === "}" || ch === "]") {
      path.pop();
      i++;
      continue;
    }

    // Bare scalar (number, true, false, null) — skip it to the
    // next structural char.
    while (
      i < len &&
      source[i] !== "," &&
      source[i] !== "}" &&
      source[i] !== "]" &&
      source[i] !== "{" &&
      source[i] !== "[" &&
      source[i] !== " " &&
      source[i] !== "\t" &&
      source[i] !== "\n" &&
      source[i] !== "\r"
    ) {
      i++;
    }
  }

  return path;
}

/**
 * Render a path array as a JSONPath-style string (`$.a.b[2].c`).
 * Keys that look like bare identifiers get the dot form; others
 * fall back to bracket-quoted form.
 */
export function formatJsonPath(path: PathSegment[]): string {
  if (path.length === 0) return "$";
  let out = "$";
  for (const seg of path) {
    if (typeof seg === "number") {
      out += `[${seg}]`;
    } else if (/^[A-Za-z_][A-Za-z0-9_]*$/.test(seg)) {
      out += `.${seg}`;
    } else {
      out += `[${JSON.stringify(seg)}]`;
    }
  }
  return out;
}

// ─── Internals ───────────────────────────────────────────────────

function skipWhitespace(src: string, from: number): number {
  let i = from;
  while (i < src.length) {
    const c = src[i];
    if (c === " " || c === "\t" || c === "\n" || c === "\r") i++;
    else break;
  }
  return i;
}

function findStringEnd(src: string, from: number): number {
  // `from` points at the opening `"`.
  for (let i = from + 1; i < src.length; i++) {
    const c = src[i];
    if (c === "\\") {
      i++;
      continue;
    }
    if (c === '"') return i;
  }
  return -1;
}

function readKey(
  src: string,
  from: number,
): { key: string; end: number } | null {
  if (src[from] !== '"') return null;
  const close = findStringEnd(src, from);
  if (close === -1) return null;
  // Use JSON.parse so escape sequences (`\n`, `A`, …) resolve
  // correctly. Falls back to the raw slice on failure.
  const raw = src.slice(from, close + 1);
  try {
    return { key: JSON.parse(raw) as string, end: close + 1 };
  } catch {
    return { key: raw.slice(1, -1), end: close + 1 };
  }
}
