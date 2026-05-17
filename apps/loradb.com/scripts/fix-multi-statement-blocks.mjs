#!/usr/bin/env node
/**
 * Mechanical fix for the `multi-statement-no-semicolon` failure mode.
 *
 * The docs author often groups two or three related examples in a single
 * <QueryCodeBlock>, separated only by a newline. The parser treats the
 * second line as continuation and reports "Extra content after the query".
 * The fix: insert `;` after the line that ended a complete statement, so
 * the script becomes a multi-statement Cypher script (the parser is
 * multi-statement-native and the playground will run each in order).
 *
 * Strategy:
 *   1. Walk every <QueryCodeBlock> in every docs file.
 *   2. For each snippet, run validate(); if no error, leave alone.
 *   3. If the error is "Extra content after the query at line N, column 1",
 *      append `;` to the *visible* end of the previous logical line
 *      (i.e. before any trailing line comment).
 *   4. Re-validate and repeat — fixed-point loop, max 8 iterations.
 *   5. If after the loop the snippet still fails, leave the file
 *      untouched for that snippet and report it.
 *
 * Writes files in place. Reports per-file: snippets fixed, snippets
 * left unfixed, total `;` inserted.
 */
import { readdir, readFile, writeFile } from "node:fs/promises";
import { join, relative, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const __dirname = fileURLToPath(new URL(".", import.meta.url));
const DOCS_ROOT = resolve(__dirname, "..", "docs");
const REPO_ROOT = resolve(__dirname, "..", "..", "..");

const args = process.argv.slice(2);
const DRY = args.includes("--dry-run");
const onlyArg = args.find((a) => a.startsWith("--only="));
const ONLY = onlyArg ? onlyArg.slice("--only=".length) : null;

async function* walk(dir) {
  for (const entry of await readdir(dir, { withFileTypes: true })) {
    const full = join(dir, entry.name);
    if (entry.isDirectory()) yield* walk(full);
    else if (entry.isFile() && entry.name.endsWith(".md")) yield full;
  }
}

// Find every <QueryCodeBlock code={String.raw`...`} /> and return
// { matchStart, bodyStart, bodyEnd } indices so we can splice fixed
// bodies back into the original text.
function findBlocks(text) {
  const blocks = [];
  let i = 0;
  const N = text.length;
  while (i < N) {
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
    if (!text.startsWith("<QueryCodeBlock", i)) {
      i += 1;
      continue;
    }
    const openProp = text.indexOf("code={String.raw`", i);
    if (openProp === -1) {
      i += 1;
      continue;
    }
    const bodyStart = openProp + "code={String.raw`".length;
    let j = bodyStart;
    while (j < text.length) {
      const ch = text[j];
      if (ch === "\\" && text[j + 1] === "`") {
        j += 2;
        continue;
      }
      if (ch === "`") break;
      j += 1;
    }
    if (j >= text.length) break;
    blocks.push({ bodyStart, bodyEnd: j });
    i = j + 1;
  }
  return blocks;
}

// Decode `\`` → `` ` ``. Other backslashes are preserved (String.raw rules).
function decodeBody(raw) {
  return raw.replace(/\\`/g, "`");
}
// Encode `` ` `` → `\``. Idempotent for already-encoded text because
// String.raw doesn't process any other escape.
function encodeBody(decoded) {
  return decoded.replace(/`/g, "\\`");
}

// Append `;` to the end of `line` immediately before any trailing line
// comment (`// …`). Returns the modified line. If the line is empty or
// already ends with `;`, returns it unchanged.
function appendSemicolonToLogicalEnd(line) {
  if (!line.trim() || /;\s*(\/\/.*)?$/.test(line)) return line;
  // Find a `//` that is NOT inside a string literal. We walk the line
  // once and bail on the first unquoted `//`.
  let inStr = null; // "'" or '"' or null
  for (let k = 0; k < line.length - 1; k += 1) {
    const ch = line[k];
    if (inStr) {
      if (ch === "\\") {
        k += 1;
        continue;
      }
      if (ch === inStr) inStr = null;
      continue;
    }
    if (ch === "'" || ch === '"') {
      inStr = ch;
      continue;
    }
    if (ch === "/" && line[k + 1] === "/") {
      // Trim trailing space *before* the comment and append `;`.
      const codePart = line.slice(0, k).replace(/\s+$/, "");
      const commentPart = line.slice(k);
      const padding = line.slice(codePart.length, k);
      return `${codePart};${padding}${commentPart}`;
    }
  }
  // No comment — append at end (trimming trailing whitespace).
  return line.replace(/\s*$/, "") + ";";
}

// One round of fix: find a single "Extra content after the query"
// diagnostic and insert a `;` before that position. Returns the new
// body, or null if no applicable fix.
//
// Use the structured `diag.line` (whole-snippet 1-based), NOT the
// "at line N, column M" embedded in the message text — that one is
// relative to the post-split statement and was the source of an
// off-by-one bug that made the fixer give up on long blocks.
function applyOneSemicolonFix(body, diag) {
  if (!/Extra content after the query/i.test(diag.message)) return null;
  const line =
    typeof diag.line === "number" && diag.line > 0 ? diag.line : 0;
  if (!(line >= 2)) return null;
  const lines = body.split("\n");
  // The previous logical line is the last non-blank line strictly
  // before `line`. (Docs sometimes leave blank visual separators.)
  let prev = line - 2; // 0-indexed
  while (prev >= 0 && !lines[prev].trim()) prev -= 1;
  if (prev < 0) return null;
  const updated = appendSemicolonToLogicalEnd(lines[prev]);
  if (updated === lines[prev]) return null;
  lines[prev] = updated;
  return lines.join("\n");
}

async function main() {
  const mod = await import("@loradb/lora-query/parser");
  if (mod.__tla) await mod.__tla;
  const { validate } = mod;

  let filesTouched = 0;
  let snippetsFixed = 0;
  let snippetsFailed = 0;
  let semisInserted = 0;
  const failures = [];

  for await (const path of walk(DOCS_ROOT)) {
    if (ONLY && !path.includes(ONLY)) continue;
    const text = await readFile(path, "utf8");
    const blocks = findBlocks(text);
    if (blocks.length === 0) continue;
    // Walk blocks back-to-front so earlier-block indices stay valid as
    // we splice fixes into the body slots.
    let next = text;
    let touchedThisFile = false;
    let fileSnippetsFixed = 0;
    let fileSnippetsFailed = 0;
    for (let bi = blocks.length - 1; bi >= 0; bi -= 1) {
      const blk = blocks[bi];
      const originalRaw = next.slice(blk.bodyStart, blk.bodyEnd);
      let body = decodeBody(originalRaw);
      const initial = await validate(body.trim());
      if (initial.length === 0) continue;
      // Only attempt the semicolon fix; leave other failure modes alone.
      if (!/Extra content after the query/i.test(initial[0]?.message ?? "")) {
        continue;
      }
      let attempts = 0;
      let lastBody = body;
      // Cap is generous because a single block can stack many short
      // RETURN statements (e.g. functions/list.md:100 has 11 lines, so
      // it needs 10 semicolon insertions to converge).
      const maxAttempts = Math.max(16, body.split("\n").length * 2);
      while (attempts < maxAttempts) {
        const diags = await validate(body.trim());
        if (diags.length === 0) break;
        const fixed = applyOneSemicolonFix(body, diags[0]);
        if (!fixed || fixed === body) break;
        lastBody = body;
        body = fixed;
        semisInserted += 1;
        attempts += 1;
      }
      const finalDiags = await validate(body.trim());
      if (finalDiags.length > 0) {
        // Didn't converge — roll back any partial changes for this
        // snippet so we don't leave a half-fixed file.
        body = decodeBody(originalRaw);
        // Adjust the running totals.
        semisInserted -= attempts;
        snippetsFailed += 1;
        fileSnippetsFailed += 1;
        failures.push({
          file: relative(REPO_ROOT, path),
          message: finalDiags[0]?.message?.split("\n")[0] ?? "(unknown)",
        });
        continue;
      }
      const encoded = encodeBody(body);
      if (encoded !== originalRaw) {
        next = next.slice(0, blk.bodyStart) + encoded + next.slice(blk.bodyEnd);
        touchedThisFile = true;
        snippetsFixed += 1;
        fileSnippetsFixed += 1;
        // Mark `lastBody` used so the linter doesn't complain.
        void lastBody;
      }
    }
    if (touchedThisFile && !DRY) {
      await writeFile(path, next, "utf8");
      filesTouched += 1;
    } else if (touchedThisFile && DRY) {
      filesTouched += 1;
    }
    if (fileSnippetsFixed > 0 || fileSnippetsFailed > 0) {
      const tag = DRY ? "[dry-run]" : "[wrote]";
      console.log(
        `${tag} ${relative(REPO_ROOT, path)}: fixed ${fileSnippetsFixed}` +
          (fileSnippetsFailed > 0 ? `, unfixed ${fileSnippetsFailed}` : ""),
      );
    }
  }

  console.log("");
  console.log("Done.");
  console.log(`  files touched:   ${filesTouched}`);
  console.log(`  snippets fixed:  ${snippetsFixed}`);
  console.log(`  semicolons:      ${semisInserted}`);
  console.log(`  snippets unfixed (left alone): ${snippetsFailed}`);
  if (failures.length > 0) {
    console.log("");
    console.log("Unfixed (other failure modes — needs separate pass):");
    for (const f of failures.slice(0, 20)) {
      console.log(`  ${f.file} — ${f.message}`);
    }
    if (failures.length > 20) {
      console.log(`  … (${failures.length - 20} more)`);
    }
  }
}

main().catch((err) => {
  console.error(err);
  process.exit(2);
});
