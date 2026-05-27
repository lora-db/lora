import React from "react";
import clsx from "clsx";
import Head from "@docusaurus/Head";

import styles from "./styles.module.scss";

/**
 * FAQ block + schema.org FAQPage JSON-LD.
 *
 * Renders a list of question/answer pairs as a native <details> /
 * <summary> accordion (no client JS required) and emits a single
 * FAQPage JSON-LD blob with each pair as a Question + Answer entity.
 *
 * Google's structured-data extractor lifts these into rich results for
 * appropriate queries; LLM crawlers treat the JSON-LD as a citation-
 * friendly source of fact-shaped statements.
 *
 *   <FAQ items={[
 *     { question: "Is LoraDB ACID?", answer: "LoraDB serializes ..." },
 *     { question: "Does it scale out?", answer: "..." },
 *   ]} />
 *
 * Only one <FAQ> per page — FAQPage is a page-level schema and adding
 * multiple emits competing assertions about what the page is about.
 *
 * Answers should be plain text (no HTML). Markdown emphasis is fine in
 * the rendered display via the `richAnswer` field, but the JSON-LD
 * always uses the plain `answer` string.
 */
export default function FAQ({ items, defaultOpen = false, className }) {
  if (!Array.isArray(items) || items.length === 0) {
    return null;
  }

  const schema = {
    "@context": "https://schema.org",
    "@type": "FAQPage",
    mainEntity: items.map((item) => ({
      "@type": "Question",
      name: item.question,
      acceptedAnswer: {
        "@type": "Answer",
        text: item.answer,
      },
    })),
  };

  return (
    <>
      <Head>
        <script type="application/ld+json">{JSON.stringify(schema)}</script>
      </Head>
      <section
        className={clsx(styles.faq, className)}
        aria-label="Frequently asked questions"
      >
        {items.map((item, idx) => (
          <details
            key={idx}
            className={styles.item}
            open={defaultOpen || item.open}
          >
            <summary className={styles.question}>{item.question}</summary>
            <div className={styles.answer}>
              {item.richAnswer ? item.richAnswer : <p>{item.answer}</p>}
            </div>
          </details>
        ))}
      </section>
    </>
  );
}
