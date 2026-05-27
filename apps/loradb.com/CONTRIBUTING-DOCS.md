# Docs contributing notes

This file is the detailed writing guide for the public docs site. Use
[`README.md`](./README.md) for local build/deploy commands, and use this file
when deciding how to write examples, inline Cypher, and validation-friendly
Markdown.

## Inline Cypher code — `<CypherCode>` vs plain backticks

The docs site ships a small React component, `<CypherCode>`, that
renders short Cypher snippets inline with Cypher-aware syntax
colouring. It's registered as a global MDX component, so you can use
it in any `.md` / `.mdx` file without an import.

### Use `<CypherCode>` for Cypher references

Keep reaching for `<CypherCode>` when the fragment is Cypher:

- clauses — `<CypherCode code="MATCH (n:Person)" />`
- functions — `<CypherCode code="temporal.today()" />`, `<CypherCode code="count(*)" />`
- property / attribute access — `<CypherCode code="n.name" />`, `<CypherCode code="dt.year" />`
- operators and expressions — `<CypherCode code="date + duration" />`
- short query fragments — `<CypherCode code="WITH n.name AS name" />`
- parameters — `<CypherCode code="$id" />`

### Keep plain backticks for everything else

Use plain backticks for non-Cypher identifiers:

- filenames — `src/theme/MDXComponents.jsx`
- CLI flags and env vars — `--port`, `LORA_SERVER_PORT`
- package names — `lora-node`, `@docusaurus/preset-classic`
- host-language identifiers — `BTreeMap`, `asyncio.to_thread`
- JSON / shell snippets

Mixing is fine. You'll often see one of each in the same sentence —
for example "pass `$id` to `db.execute_with_params` in Rust",
rendered as `<CypherCode code="$id" />` next to a plain backtick
`db.execute_with_params`.

### Two APIs

Both are supported; pick whichever reads better:

```mdx
Use <CypherCode code="temporal.today()" /> to get the current date.

Use <CypherCode>temporal.today()</CypherCode> to get the current date.
```

Prefer the `code` prop in table cells and dense reference content —
it's one attribute and always renders cleanly. Prefer children for
standalone prose if a co-author finds it easier to read.

### Markdown tables — pipe caveat

`<CypherCode>` works inside Markdown tables, but the snippet itself
must not contain a raw `|` — the table parser will split the row at
that character before the component ever sees it. For a snippet
containing a pipe (e.g. `(a)-[:T1|T2]->(b)`), fall back to plain
backticks for that one cell and note the pipe in adjacent prose.

## Block Cypher examples — `QueryCodeBlock` vs `CypherSnippet`

The docs site distinguishes runnable examples from reference
snippets:

- Use `QueryCodeBlock` only for complete, supported Cypher queries
  that should parse as standalone queries and are intended to be copied by
  readers. These blocks are collected by
  `scripts/validate-docs-cypher.mjs`.
- Use `CypherSnippet` for syntax fragments, multi-step sketches,
  intentionally unsupported examples, placeholders, and examples that
  depend on data not present in the docs example graph.

Before opening a docs PR that changes query examples, run:

```bash
corepack yarn workspace loradb-docs validate-cypher
```

The expected result is `invalid: 0`. If a new example is valid Cypher
but not meant to be executable in the docs graph, keep it as a
`CypherSnippet`.

`validate-cypher` checks parser validity, not semantic fixture data. If an
example needs prior data to return rows, include the seed query nearby or link
to the exact setup step.

## Links from docs index pages

The docs landing page (`docs/index.md`) and section index pages such
as `docs/queries/index.md` are served from route roots like `/docs`
and `/docs/queries`. Relative links from those files can resolve
somewhere surprising during the production build. Prefer explicit
site paths from index pages:

- Good: `[Parameters](/docs/queries/parameters)`
- Good: `[Query overview](/docs/queries)`
- Risky from an index page: `[Parameters](./parameters)`

Run `corepack yarn workspace loradb-docs build` after link changes.
Docusaurus fails the build on broken internal links and anchors.

## HTTP examples

When showing `POST /query`, include `params` whenever a value comes
from user input or surrounding application state:

```json
{
  "query": "MATCH (u:User) WHERE u.id = $id RETURN u",
  "params": { "id": 42 },
  "format": "rows"
}
```

Inline literals are fine for tiny, fixed examples, but avoid teaching
string interpolation as the default path.

## `llms.txt` and canonical docs

The docs build emits `/llms.txt` and `/llms-full.txt` from
`plugins/llms-txt/manifest.js`. When adding a new canonical guide,
binding page, or reference page that should be visible to AI crawlers,
add it to that manifest in the right section. Do not add every blog
post by default; keep the bundle curated and ordered for reading.

The production build logs how many entries were written. If a page is
missing from the LLM bundle, check the manifest before changing the
plugin.

### Don't

- Replace _every_ backtick. Inline code for filenames and CLI is
  fine as a backtick — wrapping those in `<CypherCode>` would
  mis-colour them as Cypher.
- Nest JSX inside `<CypherCode>`. Children must be a plain string
  (or passed via the `code` prop). Anything else is rendered
  verbatim without tokenization.
- Introduce the component into sentences that become awkward. If
  the result reads like
  `Use <CypherCode>WHERE</CypherCode> with <CypherCode>count()</CypherCode> after <CypherCode>WITH</CypherCode> to …`,
  rewrite the sentence so one or two of the inline references carry
  the weight and the rest sit in plain prose.

### Where the component lives

- Source: `src/components/CypherCode/`
  - `index.jsx` — React component
  - `tokenize.js` — lightweight Cypher tokenizer
  - `styles.module.scss` — inline-appropriate styling
- Global registration: `src/theme/MDXComponents.jsx`

Token colours reuse the same palette as fenced code blocks
(`src/styles/components/_code.scss`), so Cypher looks consistent
between inline and block contexts.

### Scope

The tokenizer is tuned for short inline snippets — it does not try
to match the full Cypher grammar. Fenced <code>```cypher</code>
blocks continue to go through the real Prism grammar registered in
`docusaurus.config.js`.

If a specific inline snippet isn't colouring correctly, check the
tokenizer's keyword list and regexes in
`src/components/CypherCode/tokenize.js`. Adding a missing keyword is
a one-line change.

## Historical posts

Blog posts under `blog/` are release notes and essays, so keep their
original claims intact when the claim was true for that release. If an
old post now teaches a stale command or limitation that could confuse a
new reader, add a short `:::note Current ...` callout that links to the
current docs page instead of rewriting the release history.
