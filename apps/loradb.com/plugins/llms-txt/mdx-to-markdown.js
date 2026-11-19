// Turns a docs page body (Docusaurus MDX) into plain Markdown.
//
// The site pages lean on a few globally registered components
// (src/theme/MDXComponents.jsx). Those render fine on loradb.com but are
// noise for anything that reads the raw text: llms-full.txt, and the docs
// bundled into the npm packages (scripts/bundle-package-docs.mjs). This
// module rewrites them into their plain-Markdown equivalent:
//
//   <QueryCodeBlock code={...} />  → ```cypher fence
//   <CypherSnippet code={...} />   → ```cypher fence
//   <CypherCode code="..." />      → `inline code`
//   <FAQ items={[...]} />          → question headings + answers
//   <HowTo name=... steps={[...]}> → numbered list
//   <Definition term=...>…</…>     → bold term + body
//   <BenchmarkSummary />, diagrams → link to the live page
//   import/export lines            → dropped
//   :::note Title … :::            → blockquote
//
// An unknown block-level component is an error, so a new one gets a
// rendering here instead of silently vanishing from the bundles.
//
// Links go through `resolveLink(href)` so the caller decides whether a
// `/docs/...` link becomes an absolute URL or a relative file path.
//
// Input is our own docs source, so component props are evaluated with
// node:vm rather than parsed by hand. Fenced code blocks and inline code
// spans are copied verbatim.

const vm = require("node:vm");

const ADMONITION_LABELS = {
  note: "Note",
  tip: "Tip",
  info: "Info",
  caution: "Caution",
  warning: "Warning",
  danger: "Danger",
};

function mdxToMarkdown(body, options = {}) {
  const ctx = {
    resolveLink: options.resolveLink || ((href) => href),
    siteUrl: (options.siteUrl || "https://loradb.com").replace(/\/$/, ""),
    pageUrl: options.pageUrl || null,
    source: options.source || "<unknown>",
  };
  let out = convert(body, ctx);
  out = stripMdxComments(out);
  out = convertAdmonitions(out);
  out = out.replace(/\n{3,}/g, "\n\n");
  return out.trim() + "\n";
}

// ── scanner ──────────────────────────────────────────────────────────

function convert(src, ctx) {
  let out = "";
  let prose = "";
  const flushProse = () => {
    out += rewriteProse(prose, ctx);
    prose = "";
  };

  let i = 0;
  while (i < src.length) {
    const atLineStart = i === 0 || src[i - 1] === "\n";

    if (atLineStart) {
      // MDX ESM (page-local component imports) has no Markdown meaning.
      const esm = /^(?:import|export)\s[^\n]*(?:\n|$)/.exec(
        src.slice(i, src.indexOf("\n", i) + 1 || src.length),
      );
      if (esm) {
        i += esm[0].length;
        continue;
      }
      const fence = matchFenceOpen(src, i);
      if (fence) {
        flushProse();
        const end = findFenceClose(src, i, fence);
        out += src.slice(i, end);
        i = end;
        continue;
      }
    }

    const ch = src[i];

    if (ch === "`") {
      let run = 0;
      while (src[i + run] === "`") run++;
      const marker = "`".repeat(run);
      const close = src.indexOf(marker, i + run);
      if (close !== -1 && !/\n\s*\n/.test(src.slice(i, close))) {
        flushProse();
        out += src.slice(i, close + run);
        i = close + run;
        continue;
      }
      prose += marker;
      i += run;
      continue;
    }

    if (ch === "<" && /[A-Z]/.test(src[i + 1] || "")) {
      const tag = parseJsx(src, i);
      if (tag) {
        flushProse();
        out += renderComponent(tag, src, i, ctx);
        i = tag.end;
        continue;
      }
    }

    prose += ch;
    i++;
  }
  flushProse();
  return out;
}

function matchFenceOpen(src, i) {
  const m = /^( {0,3})(`{3,}|~{3,})/.exec(src.slice(i, i + 64));
  return m ? { indent: m[1].length, marker: m[2] } : null;
}

function findFenceClose(src, start, fence) {
  let pos = src.indexOf("\n", start);
  if (pos === -1) return src.length;
  pos++;
  while (pos < src.length) {
    let lineEnd = src.indexOf("\n", pos);
    if (lineEnd === -1) lineEnd = src.length;
    const line = src.slice(pos, lineEnd);
    const m = /^ {0,3}(`{3,}|~{3,})\s*$/.exec(line);
    if (
      m &&
      m[1][0] === fence.marker[0] &&
      m[1].length >= fence.marker.length
    ) {
      return Math.min(lineEnd + 1, src.length);
    }
    pos = lineEnd + 1;
  }
  return src.length;
}

// ── JSX ──────────────────────────────────────────────────────────────

function parseJsx(src, start) {
  const nameMatch = /^<([A-Z][A-Za-z0-9]*)/.exec(src.slice(start, start + 64));
  if (!nameMatch) return null;
  const name = nameMatch[1];
  let i = start + nameMatch[0].length;
  const attrs = {};

  for (;;) {
    while (/\s/.test(src[i] || "")) i++;
    if (i >= src.length) return null;
    if (src.startsWith("/>", i)) {
      return { name, attrs, children: null, end: i + 2 };
    }
    if (src[i] === ">") {
      const closeTag = `</${name}>`;
      const close = src.indexOf(closeTag, i + 1);
      if (close === -1) return null;
      return {
        name,
        attrs,
        children: src.slice(i + 1, close),
        end: close + closeTag.length,
      };
    }
    const attr = /^[A-Za-z_][\w-]*/.exec(src.slice(i, i + 64));
    if (!attr) return null;
    i += attr[0].length;
    if (src[i] !== "=") {
      attrs[attr[0]] = true;
      continue;
    }
    i++;
    if (src[i] === '"' || src[i] === "'") {
      const close = src.indexOf(src[i], i + 1);
      if (close === -1) return null;
      attrs[attr[0]] = src.slice(i + 1, close);
      i = close + 1;
    } else if (src[i] === "{") {
      const close = findExpressionEnd(src, i);
      if (close === -1) return null;
      attrs[attr[0]] = { expr: src.slice(i + 1, close) };
      i = close + 1;
    } else {
      return null;
    }
  }
}

// Index of the `}` closing the expression that opens at `start`.
function findExpressionEnd(src, start) {
  const stack = ["}"];
  let i = start + 1;
  while (i < src.length) {
    const ch = src[i];
    const top = stack[stack.length - 1];

    if (top === "`") {
      if (ch === "\\") {
        i += 2;
        continue;
      }
      if (ch === "`") stack.pop();
      else if (ch === "$" && src[i + 1] === "{") {
        stack.push("}");
        i++;
      }
      i++;
      continue;
    }

    if (ch === '"' || ch === "'") {
      i++;
      while (i < src.length && src[i] !== ch) {
        if (src[i] === "\\") i++;
        i++;
      }
      i++;
      continue;
    }
    if (ch === "/" && src[i + 1] === "/") {
      const nl = src.indexOf("\n", i);
      i = nl === -1 ? src.length : nl;
      continue;
    }
    if (ch === "/" && src[i + 1] === "*") {
      const close = src.indexOf("*/", i + 2);
      i = close === -1 ? src.length : close + 2;
      continue;
    }
    if (ch === "`") stack.push("`");
    else if (ch === "{" || ch === "[" || ch === "(") {
      stack.push(ch === "{" ? "}" : ch === "[" ? "]" : ")");
    } else if (ch === "}" || ch === "]" || ch === ")") {
      if (ch !== top) return -1;
      stack.pop();
      if (stack.length === 0) return i;
    }
    i++;
  }
  return -1;
}

function attrValue(tag, key, ctx) {
  const v = tag.attrs[key];
  if (v && typeof v === "object" && "expr" in v) {
    try {
      return vm.runInNewContext(`(${v.expr})`, {}, { timeout: 1000 });
    } catch (err) {
      throw new Error(
        `[mdx-to-markdown] ${ctx.source}: could not evaluate <${tag.name} ${key}={...}>: ${err.message}`,
      );
    }
  }
  return v;
}

function renderComponent(tag, src, start, ctx) {
  const blockLevel = isBlockPosition(src, start, tag.end);

  switch (tag.name) {
    case "QueryCodeBlock":
    case "CypherSnippet": {
      const code = String(
        attrValue(tag, "code", ctx) ?? tag.children ?? "",
      ).replace(/\s+$/, "");
      return fence(code, "cypher");
    }

    case "CypherCode": {
      const code = String(attrValue(tag, "code", ctx) ?? tag.children ?? "");
      return inlineCode(code);
    }

    case "FAQ": {
      const items = attrValue(tag, "items", ctx) || [];
      return items
        .map((item) => `#### ${item.question}\n\n${item.answer}`)
        .join("\n\n");
    }

    case "HowTo": {
      const name = attrValue(tag, "name", ctx);
      const description = attrValue(tag, "description", ctx);
      const steps = attrValue(tag, "steps", ctx) || [];
      const lines = [];
      if (name) lines.push(`**${name}**`, "");
      if (description) lines.push(description, "");
      steps.forEach((step, n) => {
        const text = step.text ? `: ${step.text}` : "";
        lines.push(`${n + 1}. **${step.name}**${text}`);
      });
      return lines.join("\n");
    }

    case "Definition": {
      const term = attrValue(tag, "term", ctx);
      const text = htmlToMarkdown(
        (tag.children || "").trim() || attrValue(tag, "description", ctx) || "",
      )
        .replace(/\s*\n\s*/g, " ")
        .trim();
      return term ? `**${term}**: ${text}` : text;
    }

    case "BenchmarkSummary": {
      return `_The live benchmark summary table is rendered on the website: ${pageLink(ctx)}_`;
    }

    case "JoinTableDiagram": {
      return `_A diagram of the join-table model is shown on the website: ${pageLink(ctx)}_`;
    }

    default: {
      if (tag.children != null) {
        return rewriteProse(htmlToMarkdown(tag.children), ctx);
      }
      if (blockLevel) {
        throw new Error(
          `[mdx-to-markdown] ${ctx.source}: no plain-Markdown rendering for <${tag.name} />`,
        );
      }
      return "";
    }
  }
}

function pageLink(ctx) {
  return ctx.pageUrl ? `${ctx.siteUrl}${ctx.pageUrl}` : ctx.siteUrl;
}

function isBlockPosition(src, start, end) {
  const before = src.lastIndexOf("\n", start - 1);
  const after = src.indexOf("\n", end);
  return (
    src.slice(before + 1, start).trim() === "" &&
    src.slice(end, after === -1 ? src.length : after).trim() === ""
  );
}

function fence(code, lang) {
  let marker = "```";
  while (code.includes(marker)) marker += "`";
  return `${marker}${lang}\n${code}\n${marker}`;
}

function inlineCode(code) {
  let marker = "`";
  while (code.includes(marker)) marker += "`";
  const pad = code.startsWith("`") || code.endsWith("`") ? " " : "";
  return `${marker}${pad}${code}${pad}${marker}`;
}

// ── prose ────────────────────────────────────────────────────────────

function rewriteProse(text, ctx) {
  if (!text) return text;
  return htmlToMarkdown(text)
    .replace(/(\]\()([^)\s]+)((?:\s+"[^"]*")?\))/g, (_m, open, href, close) => {
      return open + ctx.resolveLink(href) + close;
    })
    .replace(/^(\[[^\]]+\]:\s+)(\S+)/gm, (_m, label, href) => {
      return label + ctx.resolveLink(href);
    });
}

function htmlToMarkdown(text) {
  return text
    .replace(/<strong>([\s\S]*?)<\/strong>/g, "**$1**")
    .replace(/<b>([\s\S]*?)<\/b>/g, "**$1**")
    .replace(/<em>([\s\S]*?)<\/em>/g, "_$1_")
    .replace(/<i>([\s\S]*?)<\/i>/g, "_$1_")
    .replace(/<code>([\s\S]*?)<\/code>/g, (_m, c) => inlineCode(c))
    .replace(/<a\s+href="([^"]+)"[^>]*>([\s\S]*?)<\/a>/g, "[$2]($1)");
}

function stripMdxComments(text) {
  return mapOutsideFences(text, (line) =>
    line
      .replace(/\{\/\*[\s\S]*?\*\/\}/g, "")
      .replace(/<!--\s*truncate\s*-->/g, ""),
  );
}

function convertAdmonitions(text) {
  const lines = text.split("\n");
  const out = [];
  let fenceMarker = null;
  let inAdmonition = false;

  for (const line of lines) {
    const fenceMatch = /^\s*(>\s*)?(`{3,}|~{3,})/.exec(line);
    if (fenceMarker === null && fenceMatch) {
      fenceMarker = fenceMatch[2];
    } else if (
      fenceMarker !== null &&
      fenceMatch &&
      fenceMatch[2][0] === fenceMarker[0] &&
      fenceMatch[2].length >= fenceMarker.length &&
      line.trim().replace(/^>\s*/, "") === fenceMatch[2]
    ) {
      fenceMarker = null;
      if (inAdmonition) out.push(`> ${line}`);
      else out.push(line);
      continue;
    }

    if (fenceMarker === null && fenceMatch === null) {
      const open = /^:::(\w+)\s*(.*)$/.exec(line);
      if (open && !inAdmonition) {
        const label = ADMONITION_LABELS[open[1]] || capitalize(open[1]);
        const title = open[2].trim();
        out.push(title ? `> **${label}: ${title}**` : `> **${label}**`);
        out.push(">");
        inAdmonition = true;
        continue;
      }
      if (line.trim() === ":::" && inAdmonition) {
        inAdmonition = false;
        continue;
      }
    }

    if (inAdmonition) out.push(line === "" ? ">" : `> ${line}`);
    else out.push(line);
  }
  return out.join("\n");
}

function mapOutsideFences(text, fn) {
  const lines = text.split("\n");
  let fenceMarker = null;
  return lines
    .map((line) => {
      const m = /^\s*(`{3,}|~{3,})/.exec(line);
      if (m) {
        if (fenceMarker === null) fenceMarker = m[1];
        else if (m[1][0] === fenceMarker[0] && line.trim() === m[1]) {
          fenceMarker = null;
        }
        return line;
      }
      return fenceMarker === null ? fn(line) : line;
    })
    .join("\n");
}

function capitalize(s) {
  return s.charAt(0).toUpperCase() + s.slice(1);
}

module.exports = { mdxToMarkdown };
