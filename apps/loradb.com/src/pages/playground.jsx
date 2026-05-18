import React from 'react';
import clsx from 'clsx';
import Layout from '@theme/Layout';
import Link from '@docusaurus/Link';

import PlaygroundPreview from '@site/src/components/PlaygroundPreview';
import styles from './playground.module.scss';

const PLAYGROUND_URL = 'https://play.loradb.com';

const HERO_STATS = [
  { value: '0', label: 'servers to start' },
  { value: '4', label: 'result views' },
  { value: '1', label: 'shared engine' },
];

const FEATURES = [
  {
    title: 'LoraDB query editor',
    body:
      'Monaco editing with LoraDB-aware highlighting, completion, diagnostics, formatting, and multiple query tabs.',
  },
  {
    title: 'Graph-first results',
    body:
      'Switch between graph, table, JSON, and plan views without rerunning the query. Select a node or relationship to inspect its exact payload.',
  },
  {
    title: 'Local persistence',
    body:
      'History, saved queries, snapshots, schema state, and preferences stay in browser storage for the playground origin.',
  },
  {
    title: 'Shareable query links',
    body:
      'Encode the active query into a URL for documentation, issues, pull requests, and debugging conversations.',
  },
  {
    title: 'Snapshot bridge',
    body:
      'Export a playground graph and load it from the Node, Python, Rust, WASM, Go, Ruby, or server surfaces that use the same snapshot codec.',
  },
  {
    title: 'Query analysis',
    body:
      'Use the Plan tab for diagnostics, variables, labels, relationship types, and parameters while you edit.',
  },
];

const WORKFLOW = [
  {
    kicker: '1',
    title: 'Seed a graph',
    body:
      'Paste CREATE statements, import a snapshot, or start from a saved query. The graph lives only in your browser.',
  },
  {
    kicker: '2',
    title: 'Run and inspect',
    body:
      'Use the graph canvas for shape, the table for summaries, JSON for exact tagged values, and Plan for parser/analyzer detail.',
  },
  {
    kicker: '3',
    title: 'Share or export',
    body:
      'Send a query URL when the data already exists, or pair the URL with a snapshot when someone needs the same graph.',
  },
];

const SURFACES = [
  ['Engine', 'The Rust parser, analyzer, planner, executor, and store compiled to WASM.'],
  ['Storage', 'Origin-local IndexedDB and localStorage; no account and no shared backend.'],
  ['Snapshots', 'Export and import the same snapshot format used by application bindings.'],
  ['Docs', 'Examples can link straight into a runnable browser workspace.'],
];

const BOUNDARIES = [
  'One browser-origin database per visitor.',
  'No hosted workspace, sync, or team database.',
  'No host-side parameter drawer yet.',
  'No browser WAL or filesystem paths.',
];

const DOC_LINKS = [
  ['Playground guide', '/docs/getting-started/playground'],
  ['Cookbook', '/docs/cookbook'],
  ['Query examples', '/docs/queries/examples'],
  ['WASM binding', '/docs/getting-started/wasm'],
];

export default function Playground() {
  return (
    <Layout
      title="Graph Query Playground"
      description="A browser playground for LoraDB — write queries, see the graph. Live at play.loradb.com."
      wrapperClassName={styles.wrapper}
    >
      <main className={styles.page}>
        <section className={styles.hero}>
          <div className={styles.heroInner}>
            <div className={styles.heroCopy}>
              <p className={styles.eyebrow}>
                <span className={styles.dot} />
                Live · Public preview
              </p>
              <h1 className={styles.title}>
                LoraDB queries in your browser.{' '}
                <span className={styles.titleAccent}>Graph results included.</span>
              </h1>
              <p className={styles.tagline}>
                The LoraDB playground is a browser IDE for exploring connected
                data with the same Rust engine that powers the crate and
                bindings. Write a query, inspect the graph, review analysis,
                and share the exact query by URL.
              </p>
              <div className={styles.actions}>
                <Link
                  to={PLAYGROUND_URL}
                  className={clsx(styles.btn, styles.btnPrimary)}
                >
                  Launch playground
                </Link>
                <Link
                  to="/docs/getting-started/playground"
                  className={clsx(styles.btn, styles.btnSecondary)}
                >
                  Read the guide
                </Link>
                <Link
                  to="/docs/cookbook"
                  className={clsx(styles.btn, styles.btnGhost)}
                >
                  Try recipes
                </Link>
              </div>
            </div>

            <div className={styles.heroStats} aria-label="Playground facts">
              {HERO_STATS.map((stat) => (
                <div key={stat.label} className={styles.heroStat}>
                  <strong>{stat.value}</strong>
                  <span>{stat.label}</span>
                </div>
              ))}
            </div>

            <div className={styles.heroPreview}>
              <PlaygroundPreview />
            </div>
          </div>
        </section>

        <section className={styles.workflow}>
          <div className={styles.sectionInner}>
            <p className={styles.sectionEyebrow}>How it feels</p>
            <h2 className={styles.sectionTitle}>
              A short path from idea to graph.
            </h2>
            <div className={styles.workflowGrid}>
              {WORKFLOW.map((item) => (
                <article key={item.title} className={styles.workflowStep}>
                  <span>{item.kicker}</span>
                  <h3>{item.title}</h3>
                  <p>{item.body}</p>
                </article>
              ))}
            </div>
          </div>
        </section>

        <section className={styles.features}>
          <div className={styles.sectionInner}>
            <p className={styles.sectionEyebrow}>Workbench</p>
            <h2 className={styles.sectionTitle}>
              Built for the questions that tables make awkward.
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

        <section className={styles.details}>
          <div className={styles.sectionInner}>
            <div className={styles.detailsGrid}>
              <div>
                <p className={styles.sectionEyebrow}>What runs where</p>
                <h2 className={styles.sectionTitle}>
                  Static site, local engine, no backend queue.
                </h2>
                <p className={styles.detailsLead}>
                  The playground is exported as static assets. The LoraDB
                  engine runs in a Web Worker, the graph stays in your browser,
                  and snapshots are the portable handoff when you want to move
                  the same data into application code.
                </p>
              </div>
              <div className={styles.surfaceList}>
                {SURFACES.map(([label, body]) => (
                  <div key={label} className={styles.surfaceItem}>
                    <strong>{label}</strong>
                    <span>{body}</span>
                  </div>
                ))}
              </div>
            </div>

            <div className={styles.boundaryBand}>
              <div>
                <h3>Honest preview boundaries</h3>
                <p>
                  The playground is for learning, prototyping, reproducing, and
                  sharing query shape. Production state still belongs in an app
                  binding or the HTTP server.
                </p>
              </div>
              <ul>
                {BOUNDARIES.map((item) => (
                  <li key={item}>{item}</li>
                ))}
              </ul>
            </div>
          </div>
        </section>

        <section className={styles.docsBand}>
          <div className={styles.sectionInner}>
            <p className={styles.sectionEyebrow}>Keep exploring</p>
            <h2 className={styles.sectionTitle}>
              Docs that pair well with an open playground tab.
            </h2>
            <div className={styles.docGrid}>
              {DOC_LINKS.map(([label, to]) => (
                <Link key={to} to={to} className={styles.docLink}>
                  {label}
                </Link>
              ))}
            </div>
          </div>
        </section>

        <section className={styles.cta}>
          <div className={styles.sectionInner}>
            <h2 className={styles.ctaTitle}>
              Open a graph database without opening a terminal.
            </h2>
            <p className={styles.ctaBody}>
              Start in the browser, then carry the query or snapshot into the
              binding you ship. Same query engine, same planner, same result
              shapes.
            </p>
            <div className={styles.actions}>
              <Link
                to={PLAYGROUND_URL}
                className={clsx(styles.btn, styles.btnPrimary)}
              >
                Launch playground
              </Link>
              <Link
                to="/docs/getting-started/playground"
                className={clsx(styles.btn, styles.btnSecondary)}
              >
                Read the guide
              </Link>
              <Link
                to="/blog/loradb-v0-11-playground"
                className={clsx(styles.btn, styles.btnGhost)}
              >
                Read v0.11 notes
              </Link>
            </div>
          </div>
        </section>
      </main>
    </Layout>
  );
}
