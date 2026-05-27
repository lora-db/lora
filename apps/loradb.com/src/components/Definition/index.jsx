import React from "react";
import clsx from "clsx";
import Head from "@docusaurus/Head";

import styles from "./styles.module.scss";

/**
 * Definition block + schema.org DefinedTerm JSON-LD.
 *
 * Use to mark a glossary-style explanation of a term inside prose: a
 * "property graph", "Cypher", "snapshot", "WAL", etc. The visible block
 * gets a quiet bordered treatment; the term name and the body text are
 * also serialized into a DefinedTerm JSON-LD blob in the page <head> so
 * LLM crawlers and Google's knowledge-graph extractor can lift the
 * definition cleanly.
 *
 *   <Definition term="Property graph">
 *     A data model where nodes are vertices, relationships are typed
 *     directed edges, and both can carry key-value properties.
 *   </Definition>
 *
 * Optional `description` prop overrides the text extracted from children
 * — useful when the rendered prose contains formatting (links, code)
 * that should not bleed into the JSON-LD.
 */
export default function Definition({
  term,
  description,
  termSet,
  children,
  className,
}) {
  const text =
    typeof description === "string" && description.trim()
      ? description.trim()
      : childrenToText(children).trim();

  const schema = {
    "@context": "https://schema.org",
    "@type": "DefinedTerm",
    name: term,
    description: text,
  };
  if (termSet) {
    schema.inDefinedTermSet = termSet;
  }

  return (
    <>
      <Head>
        <script type="application/ld+json">{JSON.stringify(schema)}</script>
      </Head>
      <aside
        className={clsx(styles.definition, className)}
        aria-label={`Definition: ${term}`}
      >
        <dfn className={styles.term}>{term}</dfn>
        <div className={styles.body}>{children}</div>
      </aside>
    </>
  );
}

function childrenToText(node) {
  if (node == null || typeof node === "boolean") return "";
  if (typeof node === "string" || typeof node === "number") return String(node);
  if (Array.isArray(node)) return node.map(childrenToText).join("");
  if (
    typeof node === "object" &&
    node.props &&
    node.props.children !== undefined
  ) {
    return childrenToText(node.props.children);
  }
  return "";
}
