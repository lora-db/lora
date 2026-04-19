# Docs contributing notes

## Inline Cypher code — `<CypherCode>` vs plain backticks

The docs site ships a small React component, `<CypherCode>`, that
renders short Cypher snippets inline with Cypher-aware syntax
colouring. It's registered as a global MDX component, so you can use
it in any `.md` / `.mdx` file without an import.

### Use `<CypherCode>` for Cypher references

Keep reaching for `<CypherCode>` when the fragment is Cypher:

- clauses — `<CypherCode code="MATCH (n:Person)" />`
- functions — `<CypherCode code="date()" />`, `<CypherCode code="count(*)" />`
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
Use <CypherCode code="date()" /> to get the current date.

Use <CypherCode>date()</CypherCode> to get the current date.
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
