#!/usr/bin/env node
/**
 * Extract every Cypher snippet from docs/ and validate it against the
 * real LoraDB parser (the WASM build that powers @loradb/lora-query).
 *
 * Scope:
 *   - <QueryCodeBlock code={String.raw`...`} />  — multi-line block form
 *   - <CypherCode    code="..."        />        — inline single-line form
 *   - <CypherCode>...</CypherCode>               — inline child-string form
 *
 * Out of scope:
 *   - <CypherSnippet …>   — reference-only syntax fragments
 *                           (expression-level snippets, single clauses)
 *                           that don't claim to be runnable queries.
 *
 * Output:
 *   Grouped report per file. Each failure prints the docs-file line, the
 *   snippet, the parser's diagnostic, and a caret excerpt. Process exits
 *   with code 1 if any snippet failed (so this can be a CI gate later).
 *
 * Flags:
 *   --json             machine-readable report (one JSON object per line of stderr-free stdout)
 *   --only=<glob>      only process docs paths matching this substring
 *   --quiet            print summary only, no per-snippet details
 *   --include-inline   also validate <CypherCode> snippets. Off by default
 *                      because those are mostly expression fragments
 *                      (e.g. `temporal.today()`) that don't parse as
 *                      standalone Cypher statements.
 */
import { readdir, readFile } from "node:fs/promises";
import { join, relative, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const __dirname = fileURLToPath(new URL(".", import.meta.url));
const DOCS_ROOT = resolve(__dirname, "..", "docs");
const REPO_ROOT = resolve(__dirname, "..", "..", "..");

const args = process.argv.slice(2);
const JSON_OUT = args.includes("--json");
const QUIET = args.includes("--quiet");
const SKIP_INLINE = !args.includes("--include-inline");
const onlyArg = args.find((a) => a.startsWith("--only="));
const ONLY = onlyArg ? onlyArg.slice("--only=".length) : null;

const ANSI = {
  reset: "\x1b[0m",
  dim: "\x1b[2m",
  bold: "\x1b[1m",
  red: "\x1b[31m",
  green: "\x1b[32m",
  yellow: "\x1b[33m",
  cyan: "\x1b[36m",
  magenta: "\x1b[35m",
};
const useColor = process.stdout.isTTY && !JSON_OUT;
const c = (color, s) => (useColor ? `${ANSI[color]}${s}${ANSI.reset}` : s);

async function* walk(dir) {
  for (const entry of await readdir(dir, { withFileTypes: true })) {
    const full = join(dir, entry.name);
    if (entry.isDirectory()) {
      yield* walk(full);
    } else if (entry.isFile() && entry.name.endsWith(".md")) {
      yield full;
    }
  }
}

/**
 * Walk the source character-by-character and pull out every Cypher
 * snippet, tracking its starting file offset (so we can map to a 1-based
 * line number for the report).
 *
 * Returns an array of:
 *   { kind: "block" | "inline", source: string, offset: number, raw: string }
 *
 * `source` is the Cypher with `String.raw` escapes resolved (just `\`` →
 * `` ` `` — String.raw preserves every other backslash). `raw` is the
 * verbatim slice from the docs file for the error report.
 *
 * The extractor is purpose-built for the two component shapes our docs
 * use today; it is NOT a general MDX parser. If we add a third shape,
 * extend `tryMatchBlock` / `tryMatchInline` below.
 */
function extractSnippets(text) {
  const out = [];
  let i = 0;
  const N = text.length;
  while (i < N) {
    // Skip past code fences — anything inside ``` is prose markup, not a
    // component. (Docs do still use ```rust, ```bash, ```json, etc.)
    if (text.startsWith("```", i)) {
      const end = text.indexOf("```", i + 3);
      if (end === -1) break;
      i = end + 3;
      continue;
    }
    if (text[i] !== "<") {
      i += 1;
      continue;
    }
    const block = tryMatchBlock(text, i);
    if (block) {
      out.push(block);
      i = block.endOffset;
      continue;
    }
    if (!SKIP_INLINE) {
      const inline = tryMatchInline(text, i);
      if (inline) {
        out.push(inline);
        i = inline.endOffset;
        continue;
      }
    }
    i += 1;
  }
  return out;
}

// <QueryCodeBlock code={String.raw`...`} />
//   - the props order is fixed in our docs (we always lead with `code=`).
//   - the closing form is always `\`} />` on the last line (sometimes
//     followed by other props, but we don't use any).
function tryMatchBlock(text, start) {
  const head = "<QueryCodeBlock";
  if (!text.startsWith(head, start)) return null;
  const openProp = text.indexOf("code={String.raw`", start);
  if (openProp === -1) return null;
  // Reject false positives: the prop must belong to *this* tag, so the
  // first `>` or `/>` must appear after the closing backtick.
  const bodyStart = openProp + "code={String.raw`".length;
  // Find the matching backtick. String.raw escapes only the backtick
  // itself ( \` ) — every other backslash is preserved literally.
  let j = bodyStart;
  let body = "";
  while (j < text.length) {
    const ch = text[j];
    if (ch === "\\" && text[j + 1] === "`") {
      body += "`";
      j += 2;
      continue;
    }
    if (ch === "`") break;
    body += ch;
    j += 1;
  }
  if (j >= text.length) return null;
  // After the closing backtick, expect `} />` (optionally other props
  // before the `/>` — we don't use any today, but be lenient).
  const afterTick = j + 1;
  const closeIdx = text.indexOf("/>", afterTick);
  if (closeIdx === -1) return null;
  return {
    kind: "block",
    source: body,
    offset: bodyStart,
    raw: text.slice(start, closeIdx + 2),
    endOffset: closeIdx + 2,
  };
}

// <CypherCode code="..." />  OR  <CypherCode>...</CypherCode>
//   - the inline form uses double-quoted strings; embedded `\"` decodes
//     to a literal `"` (JSX string-attribute rules). We also accept the
//     child-string form for completeness.
function tryMatchInline(text, start) {
  const head = "<CypherCode";
  if (!text.startsWith(head, start)) return null;
  // Attribute form first.
  const codeAttr = text.indexOf('code="', start);
  const closeSelf = text.indexOf("/>", start);
  const openClose = text.indexOf(">", start);
  if (
    codeAttr !== -1 &&
    closeSelf !== -1 &&
    codeAttr < closeSelf &&
    (openClose === -1 || codeAttr < openClose)
  ) {
    const bodyStart = codeAttr + 'code="'.length;
    let j = bodyStart;
    let body = "";
    while (j < text.length) {
      const ch = text[j];
      if (ch === "\\" && text[j + 1] === '"') {
        body += '"';
        j += 2;
        continue;
      }
      if (ch === '"') break;
      body += ch;
      j += 1;
    }
    if (j >= text.length) return null;
    const endIdx = text.indexOf("/>", j);
    if (endIdx === -1) return null;
    return {
      kind: "inline",
      source: body,
      offset: bodyStart,
      raw: text.slice(start, endIdx + 2),
      endOffset: endIdx + 2,
    };
  }
  // Child-string form: <CypherCode>...</CypherCode>
  if (openClose !== -1) {
    const close = text.indexOf("</CypherCode>", openClose);
    if (close === -1) return null;
    const body = text.slice(openClose + 1, close);
    // Skip JSX-expression children (`{...}`); those aren't plain strings
    // and we don't validate them.
    if (body.trimStart().startsWith("{")) return null;
    return {
      kind: "inline",
      source: body,
      offset: openClose + 1,
      raw: text.slice(start, close + "</CypherCode>".length),
      endOffset: close + "</CypherCode>".length,
    };
  }
  return null;
}

function lineColFromOffset(text, offset) {
  let line = 1;
  let col = 1;
  for (let i = 0; i < offset && i < text.length; i += 1) {
    if (text[i] === "\n") {
      line += 1;
      col = 1;
    } else {
      col += 1;
    }
  }
  return { line, col };
}

function snippetExcerpt(source, span) {
  // Build a 3-line caret view around the error span. The parser hands us
  // byte offsets into the snippet itself.
  const lines = source.split("\n");
  // Compute (line, col) inside the snippet.
  let line = 1;
  let col = 1;
  for (let i = 0; i < span.start && i < source.length; i += 1) {
    if (source[i] === "\n") {
      line += 1;
      col = 1;
    } else {
      col += 1;
    }
  }
  const before = lines.slice(Math.max(0, line - 2), line - 1);
  const at = lines[line - 1] ?? "";
  const after = lines.slice(line, line + 1);
  const gutter = (n) =>
    c("dim", String(n).padStart(3, " ") + " │ ");
  const out = [];
  let n = Math.max(1, line - before.length);
  for (const l of before) {
    out.push(gutter(n) + l);
    n += 1;
  }
  out.push(gutter(n) + at);
  out.push(c("dim", "    │ ") + " ".repeat(Math.max(0, col - 1)) + c("red", "^"));
  for (const l of after) {
    n += 1;
    out.push(gutter(n) + l);
  }
  return out.join("\n");
}

function categorise(diag, source) {
  // Coarse bucket for the summary — gives us a sense of *what* tends to
  // break across the docs without reading every diagnostic. The first
  // matching rule wins, so order them most-specific first.
  const m = diag.message ?? "";
  const lineAtErr = (() => {
    if (!diag.span || typeof diag.span.start !== "number") return "";
    const before = source.slice(0, diag.span.start);
    const lineStart = before.lastIndexOf("\n") + 1;
    const lineEnd = source.indexOf("\n", diag.span.start);
    return source.slice(lineStart, lineEnd === -1 ? source.length : lineEnd);
  })();

  if (/NOT supported|not supported|❌|invalid|wrong/i.test(lineAtErr))
    return "documented-as-invalid";
  if (/Unknown function|no such function/i.test(m)) return "unknown-function";
  if (/Unknown label/i.test(m)) return "unknown-label";
  if (/Unknown relationship type|unknown rel/i.test(m))
    return "unknown-rel-type";
  if (/Undeclared variable|not bound|out of scope/i.test(m))
    return "undeclared-variable";
  if (/Extra content after the query/i.test(m))
    return "multi-statement-no-semicolon";
  if (/Expected/i.test(m) && /::/i.test(m)) return "cast-form";
  if (/BETWEEN/i.test(lineAtErr)) return "between-keyword";
  if (/WHERE\s+(?:NOT\s+)?\w+:/i.test(lineAtErr) ||
      /\bAND\s+(?:NOT\s+)?\w+:|OR\s+(?:NOT\s+)?\w+:/i.test(lineAtErr))
    return "label-as-where-predicate";
  if (/Expected/i.test(m)) return "syntax-other";
  return "other";
}

async function main() {
  const mod = await import("@loradb/lora-query/parser");
  if (mod.__tla) await mod.__tla;
  const { validate } = mod;

  /** @type {Array<{file: string, line: number, kind: string, source: string, diags: any[]}>} */
  const failures = [];
  const stats = {
    files: 0,
    filesWithSnippets: 0,
    blocks: 0,
    inlines: 0,
    valid: 0,
    invalid: 0,
    byCategory: new Map(),
    byFile: new Map(),
  };

  for await (const path of walk(DOCS_ROOT)) {
    if (ONLY && !path.includes(ONLY)) continue;
    stats.files += 1;
    const text = await readFile(path, "utf8");
    const snippets = extractSnippets(text);
    if (snippets.length === 0) continue;
    stats.filesWithSnippets += 1;

    const fileFailures = [];
    for (const snip of snippets) {
      if (snip.kind === "block") stats.blocks += 1;
      else stats.inlines += 1;

      const trimmed = snip.source.trim();
      if (!trimmed) continue;
      const diags = await validate(trimmed);
      if (diags.length === 0) {
        stats.valid += 1;
        continue;
      }
      stats.invalid += 1;
      const { line } = lineColFromOffset(text, snip.offset);
      const rec = {
        file: relative(REPO_ROOT, path),
        line,
        kind: snip.kind,
        source: trimmed,
        diags,
      };
      failures.push(rec);
      fileFailures.push(rec);
      for (const d of diags) {
        const cat = categorise(d, trimmed);
        stats.byCategory.set(cat, (stats.byCategory.get(cat) ?? 0) + 1);
      }
    }
    if (fileFailures.length > 0) {
      stats.byFile.set(relative(REPO_ROOT, path), fileFailures.length);
    }
  }

  if (JSON_OUT) {
    process.stdout.write(
      JSON.stringify(
        {
          summary: {
            files: stats.files,
            filesWithSnippets: stats.filesWithSnippets,
            blocks: stats.blocks,
            inlines: stats.inlines,
            valid: stats.valid,
            invalid: stats.invalid,
            byCategory: Object.fromEntries(stats.byCategory),
            byFile: Object.fromEntries(stats.byFile),
          },
          failures,
        },
        null,
        2,
      ) + "\n",
    );
    process.exit(failures.length > 0 ? 1 : 0);
  }

  // Human report.
  if (!QUIET) {
    let lastFile = null;
    for (const f of failures) {
      if (f.file !== lastFile) {
        console.log("");
        console.log(c("bold", c("magenta", f.file)));
        console.log(c("dim", "─".repeat(Math.min(80, f.file.length + 4))));
        lastFile = f.file;
      }
      console.log("");
      console.log(
        `  ${c("cyan", `${f.file}:${f.line}`)} ${c("dim", `(${f.kind})`)}`,
      );
      for (const d of f.diags) {
        console.log(
          `    ${c("red", d.severity ?? "error")}: ${d.message ?? "(no message)"}`,
        );
        if (d.span && typeof d.span.start === "number") {
          console.log(snippetExcerpt(f.source, d.span).split("\n").map((l) => "    " + l).join("\n"));
        }
      }
    }
  }

  console.log("");
  console.log(c("bold", "Summary"));
  console.log(
    `  files scanned:        ${stats.files} (${stats.filesWithSnippets} with snippets)`,
  );
  console.log(`  <QueryCodeBlock>:     ${stats.blocks}`);
  console.log(`  <CypherCode> inline:  ${stats.inlines}`);
  console.log(
    `  ${c("green", `valid:   ${stats.valid}`)}    ${c(
      failures.length > 0 ? "red" : "dim",
      `invalid: ${stats.invalid}`,
    )}`,
  );
  if (stats.byCategory.size > 0) {
    console.log("");
    console.log("  by category:");
    const cats = [...stats.byCategory.entries()].sort((a, b) => b[1] - a[1]);
    for (const [k, v] of cats) {
      console.log(`    ${k.padEnd(22)} ${v}`);
    }
  }
  if (stats.byFile.size > 0) {
    console.log("");
    console.log("  by file (top 20):");
    const files = [...stats.byFile.entries()]
      .sort((a, b) => b[1] - a[1])
      .slice(0, 20);
    for (const [k, v] of files) {
      console.log(`    ${String(v).padStart(4)}  ${k}`);
    }
  }

  process.exit(failures.length > 0 ? 1 : 0);
}

main().catch((err) => {
  console.error(err);
  process.exit(2);
});
