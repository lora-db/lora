import React from "react";
import LoraQueryCodeBlock from "@site/src/components/LoraQueryCodeBlock";
import styles from "./styles.module.scss";

/**
 * Reference-only Cypher snippet.
 *
 * Use for **syntax references** that aren't standalone queries — bare
 * expressions (`count(*)`, `id(n)`), single clauses (`WHERE …`,
 * `ORDER BY …`), comma-separated function lists, type-cast forms, and
 * the like. These show up in the cheat-sheet and overview pages where
 * compactness beats runnability.
 *
 * Renders identically to {@link LoraQueryCodeBlock} today (read-only
 * editor with syntax highlighting). The semantic distinction is that
 * the docs validator (scripts/validate-docs-cypher.mjs) skips
 * `<CypherSnippet>` tags — they aren't claiming to be valid full Cypher,
 * so failing them in CI would be noise. When we add a "Run in playground"
 * action to `<QueryCodeBlock>`, this component will *not* surface it —
 * snippets are reference material, not example queries.
 */
export default function CypherSnippet(props) {
  return (
    <div className={styles.snippetWrapper} data-cypher-snippet>
      <LoraQueryCodeBlock {...props} />
    </div>
  );
}
