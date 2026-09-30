// Just enough lexing of hand-written Cypher to find `$parameters`
// (skipping strings, backtick-quoted names and comments), rename them,
// and spot constructs the translation rules avoid. The engine's own
// parser checks everything else, through `explain()`.

export interface ParamRef {
  name: string;
  start: number;
  end: number;
}

export function scanParams(text: string): ParamRef[] {
  const out: ParamRef[] = [];
  let i = 0;
  while (i < text.length) {
    const c = text[i]!;
    if (c === "'" || c === '"') {
      i = skipQuoted(text, i, c);
    } else if (c === "`") {
      const end = text.indexOf("`", i + 1);
      i = end < 0 ? text.length : end + 1;
    } else if (c === "/" && text[i + 1] === "/") {
      const end = text.indexOf("\n", i);
      i = end < 0 ? text.length : end + 1;
    } else if (c === "/" && text[i + 1] === "*") {
      const end = text.indexOf("*/", i + 2);
      i = end < 0 ? text.length : end + 2;
    } else if (c === "$") {
      const start = i;
      i++;
      let name: string;
      if (text[i] === "`") {
        const end = text.indexOf("`", i + 1);
        name = text.slice(i + 1, end < 0 ? text.length : end);
        i = end < 0 ? text.length : end + 1;
      } else {
        const m = /^[A-Za-z_][A-Za-z0-9_]*/.exec(text.slice(i));
        name = m ? m[0] : "";
        i += name.length;
      }
      if (name) out.push({ name, start, end: i });
    } else {
      i++;
    }
  }
  return out;
}

function skipQuoted(text: string, start: number, quote: string): number {
  let i = start + 1;
  while (i < text.length) {
    if (text[i] === "\\") i += 2;
    else if (text[i] === quote) return i + 1;
    else i++;
  }
  return i;
}

/** Replace every `$name` with `$<rename(name)>`. */
export function renameParams(
  text: string,
  rename: (name: string) => string,
): string {
  let out = "";
  let last = 0;
  for (const ref of scanParams(text)) {
    out += text.slice(last, ref.start) + "$" + rename(ref.name);
    last = ref.end;
  }
  return out + text.slice(last);
}

/** Code outside strings and comments, uppercased, for construct checks. */
export function codeOnly(text: string): string {
  let out = "";
  let i = 0;
  while (i < text.length) {
    const c = text[i]!;
    if (c === "'" || c === '"') {
      i = skipQuoted(text, i, c);
      out += " ";
    } else if (c === "/" && text[i + 1] === "/") {
      const end = text.indexOf("\n", i);
      i = end < 0 ? text.length : end + 1;
      out += " ";
    } else if (c === "/" && text[i + 1] === "*") {
      const end = text.indexOf("*/", i + 2);
      i = end < 0 ? text.length : end + 2;
      out += " ";
    } else {
      out += c;
      i++;
    }
  }
  return out.toUpperCase().replace(/\s+/g, " ");
}

/**
 * `text` with the same length and offsets, strings and comments blanked
 * and the insides of backtick-quoted names replaced by `_`, so structure
 * (brackets, commas, keywords) can be read without being fooled by them.
 */
export function maskLiterals(text: string): string {
  let out = "";
  let i = 0;
  while (i < text.length) {
    const c = text[i]!;
    let end: number;
    if (c === "'" || c === '"') {
      end = skipQuoted(text, i, c);
      out += " ".repeat(end - i);
    } else if (c === "`") {
      const close = text.indexOf("`", i + 1);
      end = close < 0 ? text.length : close + 1;
      out +=
        "`" + "_".repeat(Math.max(0, end - i - 2)) + (close < 0 ? "" : "`");
    } else if (c === "/" && text[i + 1] === "/") {
      const nl = text.indexOf("\n", i);
      end = nl < 0 ? text.length : nl;
      out += " ".repeat(end - i);
    } else if (c === "/" && text[i + 1] === "*") {
      const close = text.indexOf("*/", i + 2);
      end = close < 0 ? text.length : close + 2;
      out += text.slice(i, end).replace(/[^\n]/g, " ");
    } else {
      end = i + 1;
      out += c;
    }
    i = end;
  }
  return out;
}
