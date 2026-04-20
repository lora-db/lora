import React from 'react';
import clsx from 'clsx';
import Layout from '@theme/Layout';
import Link from '@docusaurus/Link';

import PlaygroundPreview from '@site/src/components/PlaygroundPreview';
import styles from './playground.module.scss';

const FEATURES = [
  {
    title: 'Write Cypher, see the graph',
    body:
      'Run MATCH, CREATE, and WITH queries in the browser. The result renders as a graph — nodes, edges, and labels you can read at a glance.',
  },
  {
    title: 'Explore shape, not just rows',
    body:
      'Follow paths, pivot on a node, and reshape the query until the answer is obvious. No table of IDs to squint at.',
  },
  {
    title: 'Share a query by URL',
    body:
      'Every query is encodable in a link. Drop it in a PR, an issue, or a doc — open it and the same graph comes back.',
  },
  {
    title: 'Same engine as the crate',
    body:
      'The playground runs LoraDB compiled to WASM — the same parser, planner, and executor you’ll run in production.',
  },
];

export default function Playground() {
  return (
    <Layout
      title="Graph Query Playground"
      description="A browser playground for LoraDB — write Cypher, see the graph. Coming soon."
      wrapperClassName={styles.wrapper}
    >
      <main className={styles.page}>
        {/* ---------- HERO ---------- */}
        <section className={styles.hero}>
          <div className={styles.heroInner}>
            <p className={styles.eyebrow}>
              <span className={styles.dot} />
              Shipping soon · Public preview
            </p>
            <h1 className={styles.title}>
              Graph Query{' '}
              <span className={styles.titleAccent}>Playground</span>.
            </h1>
            <p className={styles.tagline}>
              Write Cypher in your browser and watch the graph answer back. A
              sandbox for LoraDB built for exploring relationships, tuning
              queries, and sharing a result without spinning up a database.
            </p>
            <div className={styles.actions}>
              <span
                className={clsx(styles.btn, styles.btnPrimary, styles.btnStatic)}
                aria-disabled="true"
              >
                <span className={styles.btnDot} aria-hidden="true" />
                Coming soon
              </span>
              <Link
                to="/docs/cookbook"
                className={clsx(styles.btn, styles.btnSecondary)}
              >
                See query examples
              </Link>
              <Link
                to="https://discord.gg/vUgKb6C8Af"
                className={clsx(styles.btn, styles.btnGhost)}
              >
                Follow on Discord
              </Link>
            </div>
            <ul className={styles.heroMeta}>
              <li>
                <span className={styles.heroMetaDot} />
                Runs in the browser — no install, no account
              </li>
              <li>
                <span className={styles.heroMetaDot} />
                Shareable URLs for every query
              </li>
              <li>
                <span className={styles.heroMetaDot} />
                Backed by the same Rust engine as the crate
              </li>
            </ul>
          </div>

          <div className={styles.heroGlow} aria-hidden="true" />
        </section>

        {/* ---------- PREVIEW ---------- */}
        <section className={styles.previewSection}>
          <div className={styles.sectionInner}>
            <p className={styles.sectionEyebrow}>A peek at the interface</p>
            <h2 className={styles.sectionTitle}>
              Query on the left. Graph on the right.
            </h2>
            <div className={styles.previewFrame}>
              <PlaygroundPreview />
              <div className={styles.previewOverlay} aria-hidden="true">
                <span className={styles.previewBadge}>Preview mock</span>
              </div>
            </div>
          </div>
        </section>

        {/* ---------- FEATURES ---------- */}
        <section className={styles.features}>
          <div className={styles.sectionInner}>
            <p className={styles.sectionEyebrow}>What you’ll get</p>
            <h2 className={styles.sectionTitle}>
              Built to make connected data feel tangible.
            </h2>
            <div className={styles.featureGrid}>
              {FEATURES.map((f, i) => (
                <article key={f.title} className={styles.featureCard}>
                  <span className={styles.featureIndex}>
                    {String(i + 1).padStart(2, '0')}
                  </span>
                  <div>
                    <h3>{f.title}</h3>
                    <p>{f.body}</p>
                  </div>
                </article>
              ))}
            </div>
          </div>
        </section>

        {/* ---------- CTA ---------- */}
        <section className={styles.cta}>
          <div className={styles.sectionInner}>
            <h2 className={styles.ctaTitle}>
              A playground for graphs, not tables.
            </h2>
            <p className={styles.ctaBody}>
              Until the playground lands, the fastest way to write and run
              Cypher against LoraDB is the crate — four lines to open a
              database and run your first MATCH.
            </p>
            <div className={styles.actions}>
              <Link
                to="/docs/getting-started/installation"
                className={clsx(styles.btn, styles.btnPrimary)}
              >
                Install LoraDB
              </Link>
              <Link
                to="/blog"
                className={clsx(styles.btn, styles.btnGhost)}
              >
                Read the blog
              </Link>
            </div>
          </div>
        </section>
      </main>
    </Layout>
  );
}
