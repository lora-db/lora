import React from 'react';
import clsx from 'clsx';
import Layout from '@theme/Layout';
import Link from '@docusaurus/Link';

import BrandGraph from '@site/src/components/BrandGraph';
import StarOnGitHub from '@site/src/components/StarOnGitHub';
import styles from './index.module.scss';

const SAMPLE = `MATCH (a:Agent)-[:REMEMBERS]->(c:Context)
      -[:ABOUT]->(e:Entity)
WHERE c.updated_at > datetime() - duration('PT1H')
RETURN e.id, collect(c.summary) AS recent_context`;

// Multi-language quickstart snippets. Intentionally aligned in shape
// across tabs — open a database, run a CREATE, run a MATCH — so a
// reader can compare bindings at a glance.
const QUICKSTART_TABS = [
  {
    id: 'node',
    label: 'Node.js',
    file: 'quickstart.ts',
    code: `import { createDatabase } from '@loradb/lora-node';

const db = await createDatabase();

await db.execute(
  "CREATE (:Person {name: 'Ada'})-[:INFLUENCED]->(:Person {name: 'Grace'})"
);

const result = await db.execute(
  "MATCH (a)-[:INFLUENCED]->(b) RETURN a.name, b.name"
);

console.log(result.rows);`,
  },
  {
    id: 'python',
    label: 'Python',
    file: 'quickstart.py',
    code: `from lora_python import Database

db = Database.create()

db.execute(
    "CREATE (:Person {name: 'Ada'})-[:INFLUENCED]->(:Person {name: 'Grace'})"
)

result = db.execute(
    "MATCH (a)-[:INFLUENCED]->(b) RETURN a.name, b.name"
)

print(result["rows"])`,
  },
  {
    id: 'wasm',
    label: 'WASM',
    file: 'quickstart.ts',
    code: `import { createDatabase } from '@loradb/lora-wasm';

const db = await createDatabase();

await db.execute(
  "CREATE (:Person {name: 'Ada'})-[:INFLUENCED]->(:Person {name: 'Grace'})"
);

const result = await db.execute(
  "MATCH (a)-[:INFLUENCED]->(b) RETURN a.name, b.name"
);

console.log(result.rows);`,
  },
  {
    id: 'go',
    label: 'Go',
    file: 'quickstart.go',
    code: `import lora "github.com/lora-db/lora/crates/lora-go"

db, _ := lora.New()
defer db.Close()

db.Execute(
    "CREATE (:Person {name: 'Ada'})-[:INFLUENCED]->(:Person {name: 'Grace'})",
    nil,
)

r, _ := db.Execute(
    "MATCH (a)-[:INFLUENCED]->(b) RETURN a.name, b.name",
    nil,
)

fmt.Println(r.Rows)`,
  },
  {
    id: 'ruby',
    label: 'Ruby',
    file: 'quickstart.rb',
    code: `require "lora_ruby"

db = LoraRuby::Database.create

db.execute(
  "CREATE (:Person {name: 'Ada'})-[:INFLUENCED]->(:Person {name: 'Grace'})"
)

result = db.execute(
  "MATCH (a)-[:INFLUENCED]->(b) RETURN a.name, b.name"
)

puts result["rows"]`,
  },
];

const USE_CASES = [
  {
    title: 'AI agents & LLM pipelines',
    body: 'Tools, entities, observations and decisions as a live graph. Retrieval becomes a pattern match, not a similarity score.',
    icon: 'agent',
  },
  {
    title: 'Context & memory systems',
    body: 'Model claims, evidence, citations, and contradictions as typed edges. Ask “why do we believe this?” as a traversal.',
    icon: 'memory',
  },
  {
    title: 'Robotics & scene graphs',
    body: 'Objects, rooms, and affordances as nodes. Plan queries run inside the controller — no network hop, no migration.',
    icon: 'robot',
  },
  {
    title: 'Event pipelines & streams',
    body: 'Resolve entities, infer relationships, and enrich events in-process with Cypher rules that read top-to-bottom.',
    icon: 'stream',
  },
  {
    title: 'Real-time reasoning',
    body: 'Fraud signals, lineage, access inference, recommendations — decisions that look across entities in one query.',
    icon: 'spark',
  },
  {
    title: 'Embedded graph storage',
    body: 'A graph data structure inside your own process. No service to deploy, no protocol to speak, no daemon to babysit.',
    icon: 'cube',
  },
];

const VALUE_PROPS = [
  {
    title: 'Relationships are first-class',
    body: 'Edges are typed, directed, and property-bearing. Traversal is O(degree), not a stack of self-joins.',
  },
  {
    title: 'Cypher where it counts',
    body: 'A pragmatic subset of Cypher — MATCH, WITH, WHERE, CREATE, RETURN. Short queries, readable intent.',
  },
  {
    title: 'Schema-free by design',
    body: 'Add a label, an edge type, or a property by writing it. No ALTER, no migration, no restart.',
  },
  {
    title: 'Small enough to read',
    body: 'A compiler-style pipeline of focused crates from parser to executor. If the database matters to your product, you should be able to read it.',
  },
];

function Icon({ name }) {
  // Tiny, monochrome, currentColor SVGs. Deliberately abstract so
  // they feel system-like rather than stock-illustration.
  const common = {
    viewBox: '0 0 24 24',
    fill: 'none',
    stroke: 'currentColor',
    strokeWidth: 1.6,
    strokeLinecap: 'round',
    strokeLinejoin: 'round',
    'aria-hidden': true,
  };
  switch (name) {
    case 'agent':
      return (
        <svg {...common}>
          <circle cx="12" cy="12" r="3.2" />
          <circle cx="5" cy="6" r="1.8" />
          <circle cx="19" cy="6" r="1.8" />
          <circle cx="5" cy="18" r="1.8" />
          <circle cx="19" cy="18" r="1.8" />
          <path d="M7 7l3 3M17 7l-3 3M7 17l3-3M17 17l-3-3" />
        </svg>
      );
    case 'memory':
      return (
        <svg {...common}>
          <path d="M4 7c0-1.7 3.6-3 8-3s8 1.3 8 3-3.6 3-8 3-8-1.3-8-3z" />
          <path d="M4 7v5c0 1.7 3.6 3 8 3s8-1.3 8-3V7" />
          <path d="M4 12v5c0 1.7 3.6 3 8 3s8-1.3 8-3v-5" />
        </svg>
      );
    case 'robot':
      return (
        <svg {...common}>
          <rect x="5" y="8" width="14" height="11" rx="2.5" />
          <path d="M12 4v4M9 13h.01M15 13h.01M9 17h6" />
          <path d="M3 13v2M21 13v2" />
        </svg>
      );
    case 'stream':
      return (
        <svg {...common}>
          <path d="M3 7h8M3 12h14M3 17h10" />
          <circle cx="13" cy="7" r="1.5" />
          <circle cx="19" cy="12" r="1.5" />
          <circle cx="15" cy="17" r="1.5" />
        </svg>
      );
    case 'spark':
      return (
        <svg {...common}>
          <path d="M12 3v5M12 16v5M3 12h5M16 12h5" />
          <path d="M6.2 6.2l3 3M14.8 14.8l3 3M6.2 17.8l3-3M14.8 9.2l3-3" />
          <circle cx="12" cy="12" r="2" />
        </svg>
      );
    case 'cube':
      return (
        <svg {...common}>
          <path d="M12 3l8 4.5v9L12 21l-8-4.5v-9L12 3z" />
          <path d="M4 7.5L12 12l8-4.5M12 12v9" />
        </svg>
      );
    default:
      return null;
  }
}

export default function Home() {
  const [activeTab, setActiveTab] = React.useState(QUICKSTART_TABS[0].id);
  const activeSnippet =
    QUICKSTART_TABS.find((t) => t.id === activeTab) ?? QUICKSTART_TABS[0];

  return (
    <Layout
      title="The embedded graph database for connected systems"
      description="LoraDB is an embedded, Rust-native graph database with a Cypher-like engine — built for AI agents, robotics, and context-rich systems that reason over connected data."
      wrapperClassName={styles.homeWrapper}
    >
      <main className={styles.home}>
        {/* ---------- HERO ---------- */}
        <section className={styles.hero}>
          <div className={styles.heroGrid}>
            <div className={styles.heroCopy}>
              <p className={styles.eyebrow}>
                <span className={styles.dot} />
                Embedded · Rust · Cypher-like
              </p>
              <h1 className={styles.title}>
                The graph database for{' '}
                <span className={styles.titleAccent}>connected systems</span>.
              </h1>
              <p className={styles.tagline}>
                LoraDB is an in-process graph store with a Cypher-like query
                engine — small enough to embed in an agent, a robot, or a
                stream processor, and expressive enough to model the
                relationships those systems actually depend on.
              </p>
              <div className={styles.actions}>
                <Link
                  to="/docs/getting-started/installation"
                  className={clsx(styles.btn, styles.btnPrimary)}
                >
                  Quickstart
                  <svg
                    width="14"
                    height="14"
                    viewBox="0 0 24 24"
                    fill="none"
                    stroke="currentColor"
                    strokeWidth="2.2"
                    strokeLinecap="round"
                    strokeLinejoin="round"
                    aria-hidden="true"
                  >
                    <path d="M5 12h14M13 5l7 7-7 7" />
                  </svg>
                </Link>
                <Link
                  to="/docs"
                  className={clsx(styles.btn, styles.btnSecondary)}
                >
                  Read the docs
                </Link>
                <StarOnGitHub />
              </div>
              <ul className={styles.heroMeta}>
                <li>
                  <span className={styles.heroMetaDot} />
                  Node.js · Python · WASM · Go · Ruby
                </li>
                <li>
                  <span className={styles.heroMetaDot} />
                  Zero daemons · runs in your process
                </li>
                <li>
                  <span className={styles.heroMetaDot} />
                  Open source · readable end-to-end
                </li>
              </ul>
            </div>

            <div className={styles.heroVisual}>
              <div className={styles.heroVisualInner}>
                <BrandGraph />
                <div className={styles.codeCard} aria-label="Example Cypher query">
                  <div className={styles.codeCardHeader}>
                    <span className={styles.codeDots} aria-hidden="true">
                      <span />
                      <span />
                      <span />
                    </span>
                    <span className={styles.codeCardTitle}>
                      context.cypher
                    </span>
                  </div>
                  <pre className={styles.codeCardBody}>
                    <code>{SAMPLE}</code>
                  </pre>
                </div>
              </div>
            </div>
          </div>

          <div className={styles.heroGlow} aria-hidden="true" />
        </section>

        {/* ---------- WHY NOW ---------- */}
        <section className={styles.whyNow}>
          <div className={styles.sectionInner}>
            <p className={styles.sectionEyebrow}>Why now</p>
            <h2 className={styles.sectionTitle}>
              Modern systems are graphs.{' '}
              <span className={styles.mutedHeading}>
                Most databases aren’t.
              </span>
            </h2>
            <div className={styles.whyNowGrid}>
              <article className={styles.whyNowCard}>
                <h3>Relational stores fight relational questions</h3>
                <p>
                  “Everything reachable from here” turns into self-joins
                  stacked on self-joins. The planner guesses how to walk a
                  graph it doesn’t know is a graph.
                </p>
              </article>
              <article className={styles.whyNowCard}>
                <h3>Document stores fight evolving relationships</h3>
                <p>
                  Nesting works until ownership isn’t strict. Bidirectional
                  edges and many-to-many links push consistency into
                  application code.
                </p>
              </article>
              <article className={styles.whyNowCard}>
                <h3>Graph platforms are often disproportionate</h3>
                <p>
                  A service, a protocol, and a TCO that only pays off at
                  scale — when all you wanted was a graph data structure
                  next to the code that uses it.
                </p>
              </article>
            </div>
            <p className={styles.whyNowFooter}>
              LoraDB is the option that was missing in the other direction —
              the one you reach for when the graph belongs{' '}
              <em>inside</em> your process.
            </p>
          </div>
        </section>

        {/* ---------- USE CASES ---------- */}
        <section className={styles.useCases}>
          <div className={styles.sectionInner}>
            <p className={styles.sectionEyebrow}>Built for</p>
            <h2 className={styles.sectionTitle}>
              Systems that reason over connected, evolving context.
            </h2>
            <div className={styles.useCaseGrid}>
              {USE_CASES.map((c) => (
                <article key={c.title} className={styles.useCase}>
                  <div className={styles.useCaseIcon} aria-hidden="true">
                    <Icon name={c.icon} />
                  </div>
                  <h3>{c.title}</h3>
                  <p>{c.body}</p>
                </article>
              ))}
            </div>
          </div>
        </section>

        {/* ---------- VALUE PROPS ---------- */}
        <section className={styles.values}>
          <div className={styles.sectionInner}>
            <p className={styles.sectionEyebrow}>Developer value</p>
            <h2 className={styles.sectionTitle}>
              A graph model that matches the shape of your code.
            </h2>
            <div className={styles.valueGrid}>
              {VALUE_PROPS.map((v, i) => (
                <article key={v.title} className={styles.valueCard}>
                  <span className={styles.valueIndex}>
                    {String(i + 1).padStart(2, '0')}
                  </span>
                  <div>
                    <h3>{v.title}</h3>
                    <p>{v.body}</p>
                  </div>
                </article>
              ))}
            </div>
          </div>
        </section>

        {/* ---------- START IN A MINUTE ---------- */}
        <section className={styles.start}>
          <div className={styles.sectionInner}>
            <div className={styles.startGrid}>
              <div className={styles.startCopy}>
                <p className={styles.sectionEyebrow}>Start in a minute</p>
                <h2 className={styles.sectionTitle}>
                  Add the crate. Open a database. Write a query.
                </h2>
                <p className={styles.startBody}>
                  There’s no server to stand up, no protocol to speak. Opening
                  a LoraDB is a function call — in Node.js, Python, WASM, Go,
                  or Ruby. Same Cypher, same result shape, across every
                  binding.
                </p>
                <div className={styles.actions}>
                  <Link
                    to="/docs/getting-started/installation"
                    className={clsx(styles.btn, styles.btnPrimary)}
                  >
                    Install
                  </Link>
                  <Link
                    to="/docs/cookbook"
                    className={clsx(styles.btn, styles.btnSecondary)}
                  >
                    Cookbook
                  </Link>
                  <Link
                    to="/docs/why"
                    className={clsx(styles.btn, styles.btnGhost)}
                  >
                    Why LoraDB
                  </Link>
                </div>
              </div>

              <div className={styles.startSnippet}>
                <div
                  className={styles.codeCard}
                  role="region"
                  aria-label="Quickstart code example"
                >
                  <div className={styles.codeCardHeader}>
                    <span className={styles.codeDots} aria-hidden="true">
                      <span />
                      <span />
                      <span />
                    </span>
                    <div
                      className={styles.langTabs}
                      role="tablist"
                      aria-label="Language"
                    >
                      {QUICKSTART_TABS.map((t) => (
                        <button
                          key={t.id}
                          type="button"
                          role="tab"
                          aria-selected={activeTab === t.id}
                          tabIndex={activeTab === t.id ? 0 : -1}
                          id={`lang-tab-${t.id}`}
                          aria-controls={`lang-panel-${t.id}`}
                          className={clsx(
                            styles.langTab,
                            activeTab === t.id && styles.langTabActive,
                          )}
                          onClick={() => setActiveTab(t.id)}
                        >
                          {t.label}
                        </button>
                      ))}
                    </div>
                    <span className={styles.codeCardTitle}>
                      {activeSnippet.file}
                    </span>
                  </div>
                  <pre
                    className={styles.codeCardBody}
                    id={`lang-panel-${activeSnippet.id}`}
                    role="tabpanel"
                    aria-labelledby={`lang-tab-${activeSnippet.id}`}
                  >
                    <code>{activeSnippet.code}</code>
                  </pre>
                </div>
              </div>
            </div>
          </div>
        </section>

        {/* ---------- FINAL CTA ---------- */}
        <section className={styles.finalCta}>
          <div className={styles.sectionInner}>
            <h2 className={styles.finalCtaTitle}>
              The graph belongs inside your process.
            </h2>
            <p className={styles.finalCtaBody}>
              Build on a graph database that was designed for agents,
              memory pipelines, and event-driven systems — not retrofitted for
              them.
            </p>
            <div className={styles.actions}>
              <Link
                to="/docs"
                className={clsx(styles.btn, styles.btnPrimary)}
              >
                Start reading the docs
              </Link>
              <StarOnGitHub size="lg" />
            </div>
          </div>
        </section>
      </main>
    </Layout>
  );
}
