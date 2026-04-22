import React from 'react';
import clsx from 'clsx';
import Layout from '@theme/Layout';
import Link from '@docusaurus/Link';

import CypherCode from '@site/src/components/CypherCode';
import styles from './features.module.scss';

// -------------------------------------------------------------------
// Static content
// -------------------------------------------------------------------

const FEATURE_GROUPS = [
  {
    icon: 'pipeline',
    title: 'Compiler-style query engine',
    body:
      'Every query walks a real pipeline — PEG parser, semantic analyzer, logical and physical plans, executor — written from scratch in Rust and inspectable end-to-end.',
  },
  {
    icon: 'graph',
    title: 'Property graph model',
    body:
      'Nodes carry zero or more labels. Relationships are typed, directed, and property-bearing. Properties on either side hold scalars, lists, maps, temporals, or spatial points.',
  },
  {
    icon: 'cypher',
    title: 'Read and write Cypher',
    body:
      'MATCH, OPTIONAL MATCH, WHERE, RETURN, WITH, ORDER BY, SKIP, LIMIT, DISTINCT, UNWIND, UNION — alongside CREATE, MERGE (with ON MATCH / ON CREATE), SET, REMOVE, DELETE, DETACH DELETE.',
  },
  {
    icon: 'paths',
    title: 'Variable-length paths',
    body:
      'Bounded and unbounded path quantifiers, zero-hop matches, cycle avoidance, path binding, shortestPath() and allShortestPaths() — without bolting on a separate traversal API.',
  },
  {
    icon: 'agg',
    title: 'Aggregation and expressions',
    body:
      'count, sum, avg, min, max, collect, stdev, percentileCont and friends. Plus arithmetic, regex (=~), CASE, list and pattern comprehension, REDUCE, EXISTS subqueries, and map projection.',
  },
  {
    icon: 'temporal',
    title: 'Temporal and spatial types',
    body:
      'Date, Time, LocalTime, DateTime, LocalDateTime, Duration with arithmetic and truncation. 2D and 3D Points in Cartesian and WGS-84, with Euclidean and Haversine distance.',
  },
  {
    icon: 'functions',
    title: '60+ built-in functions',
    body:
      'String, math (full trigonometry), list, type conversion, entity introspection, path, temporal, and spatial. No procedure plugins to install — they ship in the engine.',
  },
  {
    icon: 'formats',
    title: 'Multiple result shapes',
    body:
      'Choose rows, rowArrays, graph, or combined output per query. The same engine speaks the format that fits your client — table view, raw arrays, or the actual subgraph.',
  },
];

// Pipeline stages. Each one corresponds to a real crate in the
// workspace; keeping the names accurate lets a curious reader trace
// from this page into the source tree.
const PIPELINE_STAGES = [
  {
    step: '01',
    name: 'Parse',
    crate: 'lora-parser',
    body: 'A PEG grammar lifts Cypher text into a typed AST with source spans.',
  },
  {
    step: '02',
    name: 'Analyze',
    crate: 'lora-analyzer',
    body: 'Variable scoping, label and type validation against live graph state, function resolution.',
  },
  {
    step: '03',
    name: 'Compile',
    crate: 'lora-compiler',
    body: 'Lower the resolved IR into a logical plan, optimize (filter push-down), then a physical plan.',
  },
  {
    step: '04',
    name: 'Execute',
    crate: 'lora-executor',
    body: 'Interpret the physical plan against the in-memory store and project results in the requested shape.',
  },
];

// Each Cypher coverage block uses CypherCode for the snippet so the
// tokens render with the same colours as fenced code blocks elsewhere
// on the site.
const CYPHER_COVERAGE = [
  {
    label: 'Pattern matching',
    snippet:
      "MATCH (a:Person)-[:KNOWS]->(b:Person)\nWHERE a.city = 'Berlin'\nRETURN a.name, collect(b.name) AS friends",
  },
  {
    label: 'Writing data',
    snippet:
      "MERGE (u:User {email: $email})\nON CREATE SET u.created = datetime()\nON MATCH  SET u.last_seen = datetime()",
  },
  {
    label: 'Variable-length paths',
    snippet:
      'MATCH p = shortestPath(\n  (a:Stop {code: $from})-[:CONNECTS*..6]->(b:Stop {code: $to})\n)\nRETURN length(p) AS hops, [n IN nodes(p) | n.code] AS via',
  },
  {
    label: 'Aggregation pipelines',
    snippet:
      "MATCH (u:User)-[:PLACED]->(o:Order {status: 'paid'})\nWITH u, count(o) AS orders, sum(o.total) AS spend\nWHERE orders >= 3\nRETURN u.email, orders, spend ORDER BY spend DESC",
  },
  {
    label: 'Temporal predicates',
    snippet:
      "MATCH (e:Event)\nWHERE e.at >= datetime() - duration('P7D')\nRETURN date(e.at) AS day, count(*) AS events\nORDER BY day",
  },
  {
    label: 'Spatial distance',
    snippet:
      "WITH point({latitude: 52.52, longitude: 13.405}) AS origin\nMATCH (s:Store)\nWHERE distance(s.loc, origin) < 5000\nRETURN s.name, distance(s.loc, origin) AS metres\nORDER BY metres",
  },
];

const SURFACES = [
  {
    id: 'rust',
    label: 'Rust crate',
    file: 'main.rs',
    note: 'lora-database',
    code: `use lora_database::Database;

let db = Database::in_memory();
db.execute("CREATE (:User {name: 'Ada'})", None)?;

let result = db.execute(
    "MATCH (u:User) RETURN u.name",
    None,
)?;`,
  },
  {
    id: 'http',
    label: 'HTTP server',
    file: 'shell',
    note: 'lora-server',
    code: `# Health check
curl http://127.0.0.1:4747/health

# Run a query
curl -s http://127.0.0.1:4747/query \\
  -H 'content-type: application/json' \\
  -d '{
    "query": "MATCH (u:User) RETURN u.name",
    "format": "rows"
  }'`,
  },
  {
    id: 'node',
    label: 'Node.js',
    file: 'app.ts',
    note: 'lora-node · prototype',
    code: `import { createDatabase } from 'lora-node';

const db = await createDatabase();

await db.execute(
  "CREATE (:User {name: 'Ada'})"
);

const result = await db.execute(
  "MATCH (u:User) RETURN u.name"
);`,
  },
  {
    id: 'python',
    label: 'Python',
    file: 'app.py',
    note: 'lora-python · prototype',
    code: `from lora_python import Database

db = Database.create()

db.execute("CREATE (:User {name: 'Ada'})")

result = db.execute(
    "MATCH (u:User) RETURN u.name"
)`,
  },
  {
    id: 'wasm',
    label: 'WebAssembly',
    file: 'main.ts',
    note: 'lora-wasm · prototype',
    code: `import { createDatabase } from 'lora-wasm';

const db = await createDatabase();
await db.execute("CREATE (:User {name: 'Ada'})");

const result = await db.execute(
  "MATCH (u:User) RETURN u.name"
);`,
  },
  {
    id: 'go',
    label: 'Go',
    file: 'main.go',
    note: 'lora-go · prototype',
    code: `import lora "github.com/lora-db/lora/crates/lora-go"

db, _ := lora.New()
defer db.Close()

db.Execute("CREATE (:User {name: 'Ada'})", nil)

r, _ := db.Execute(
    "MATCH (u:User) RETURN u.name",
    nil,
)`,
  },
  {
    id: 'ruby',
    label: 'Ruby',
    file: 'app.rb',
    note: 'lora-ruby · prototype',
    code: `require "lora_ruby"

db = LoraRuby::Database.create
db.execute("CREATE (:User {name: 'Ada'})")

result = db.execute(
  "MATCH (u:User) RETURN u.name"
)`,
  },
];

const WORKLOADS = [
  {
    title: 'Local development',
    body: 'Spin up a graph in a function call. No daemon to babysit, no docker-compose to maintain.',
  },
  {
    title: 'Test fixtures',
    body: 'Seed a graph per test, run assertions, drop it. Each test gets a clean database in microseconds.',
  },
  {
    title: 'Prototypes',
    body: 'Model a domain in Cypher before committing to a schema. Add labels and edge types by writing them.',
  },
  {
    title: 'Notebooks',
    body: 'Drive a graph from Python, Ruby, or WASM in a notebook. Inspect rows, the subgraph, or both.',
  },
  {
    title: 'Embedded apps',
    body: 'Ship a graph alongside the binary. Plan queries, scene graphs, and rules live next to the code.',
  },
  {
    title: 'Browser & edge',
    body: 'Compile to WebAssembly and run a graph in the same context as the UI or the request handler.',
  },
];

const BOUNDARIES = [
  {
    title: 'In-memory only',
    body: 'The store is BTreeMap-backed and lives in process memory. No persistence yet — restart, fresh graph.',
  },
  {
    title: 'No property indexes',
    body: 'Predicates are evaluated by scan. Plenty fast for the workloads above; not a substitute for an indexed planner.',
  },
  {
    title: 'Single global lock',
    body: 'The executor holds one mutex per database. Great for embedded and per-request use; not for high-fan-out concurrency.',
  },
  {
    title: 'No auth or TLS in core',
    body: 'lora-server is meant to live behind your own ingress. The crate has no auth surface — you control the host process.',
  },
];

// -------------------------------------------------------------------
// Inline icons. Same authoring rules as the homepage: monochrome,
// currentColor, abstract enough to feel system-like.
// -------------------------------------------------------------------

function Icon({ name }) {
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
    case 'pipeline':
      return (
        <svg {...common}>
          <circle cx="4" cy="12" r="2" />
          <circle cx="12" cy="12" r="2" />
          <circle cx="20" cy="12" r="2" />
          <path d="M6 12h4M14 12h4" />
        </svg>
      );
    case 'graph':
      return (
        <svg {...common}>
          <circle cx="6" cy="6" r="2" />
          <circle cx="18" cy="6" r="2" />
          <circle cx="6" cy="18" r="2" />
          <circle cx="18" cy="18" r="2" />
          <circle cx="12" cy="12" r="2" />
          <path d="M7.5 7.5l3 3M16.5 7.5l-3 3M7.5 16.5l3-3M16.5 16.5l-3-3" />
        </svg>
      );
    case 'cypher':
      return (
        <svg {...common}>
          <path d="M4 6h16M4 12h10M4 18h16" />
          <path d="M17 11l3 1.5L17 14" />
        </svg>
      );
    case 'paths':
      return (
        <svg {...common}>
          <circle cx="5" cy="6" r="1.6" />
          <circle cx="12" cy="12" r="1.6" />
          <circle cx="19" cy="6" r="1.6" />
          <circle cx="19" cy="18" r="1.6" />
          <path d="M6.4 6.6L10.6 11.4M13.4 12.6L17.6 16.6M13 11l5-4" />
        </svg>
      );
    case 'agg':
      return (
        <svg {...common}>
          <path d="M4 20V8M10 20v-6M16 20v-9M22 20v-4" />
          <path d="M3 20h20" />
        </svg>
      );
    case 'temporal':
      return (
        <svg {...common}>
          <circle cx="12" cy="13" r="7" />
          <path d="M12 9v4l2.5 2M9 3h6" />
        </svg>
      );
    case 'functions':
      return (
        <svg {...common}>
          <path d="M9 4c-2 0-3 1-3 4v2H4M9 4c2 0 3 1 3 4v8c0 3 1 4 3 4M15 10h5" />
        </svg>
      );
    case 'formats':
      return (
        <svg {...common}>
          <rect x="3" y="4" width="8" height="16" rx="1.5" />
          <rect x="13" y="4" width="8" height="7" rx="1.5" />
          <rect x="13" y="13" width="8" height="7" rx="1.5" />
        </svg>
      );
    case 'rust':
      return (
        <svg {...common}>
          <path d="M12 4l2 2 3 .4 1.5 2.6L21 11l-1.5 2-.5 3-3 1L14 19l-2 1-2-1-2-1.4-3-1L4 14l-1.5-2L3 9l1.5-2.6L8 6l2-2 2 0z" />
          <circle cx="12" cy="12" r="3" />
        </svg>
      );
    case 'http':
      return (
        <svg {...common}>
          <circle cx="12" cy="12" r="9" />
          <path d="M3 12h18M12 3c2.5 3 2.5 15 0 18M12 3c-2.5 3-2.5 15 0 18" />
        </svg>
      );
    case 'node':
      return (
        <svg {...common}>
          <path d="M12 3l8 4.5v9L12 21l-8-4.5v-9L12 3z" />
          <path d="M12 8v6M9 11h0M15 11l-2 2-2-2" />
        </svg>
      );
    case 'python':
      return (
        <svg {...common}>
          <path d="M9 4h5a3 3 0 0 1 3 3v3H9a3 3 0 0 0-3 3v3a3 3 0 0 0 3 3h3" />
          <path d="M15 20h-5a3 3 0 0 1-3-3v-3h8a3 3 0 0 0 3-3V8a3 3 0 0 0-3-3h-3" />
          <circle cx="11" cy="7" r="0.6" fill="currentColor" stroke="none" />
          <circle cx="13" cy="17" r="0.6" fill="currentColor" stroke="none" />
        </svg>
      );
    case 'wasm':
      return (
        <svg {...common}>
          <rect x="3" y="6" width="18" height="12" rx="2" />
          <path d="M7 10l1.5 4 1.5-3 1.5 3 1.5-4M16 10l1 4 1-4" />
        </svg>
      );
    case 'go':
      return (
        <svg {...common}>
          <ellipse cx="12" cy="12" rx="8" ry="6" />
          <circle cx="9.5" cy="11" r="1" fill="currentColor" stroke="none" />
          <circle cx="14.5" cy="11" r="1" fill="currentColor" stroke="none" />
          <path d="M3 10h2M3 13h2M19 10h2M19 13h2" />
        </svg>
      );
    case 'ruby':
      return (
        <svg {...common}>
          <path d="M7 4h10l4 6-9 10-9-10 4-6z" />
          <path d="M7 4l5 6M17 4l-5 6M3 10h18" />
        </svg>
      );
    default:
      return null;
  }
}

function ArrowIcon() {
  return (
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
  );
}

const SURFACE_ICONS = {
  rust: 'rust',
  http: 'http',
  node: 'node',
  python: 'python',
  wasm: 'wasm',
  go: 'go',
  ruby: 'ruby',
};

// -------------------------------------------------------------------
// Page
// -------------------------------------------------------------------

export default function Features() {
  const [activeSurface, setActiveSurface] = React.useState(SURFACES[0].id);
  const surface = SURFACES.find((s) => s.id === activeSurface) ?? SURFACES[0];

  return (
    <Layout
      title="Features"
      description="LoraDB is an in-memory graph database with a full Cypher engine, written in Rust. Explore the query pipeline, language coverage, runtime surfaces, and the workloads it's built for."
      wrapperClassName={styles.wrapper}
    >
      <main className={styles.page}>
        {/* ---------- HERO ---------- */}
        <section className={styles.hero} aria-labelledby="features-hero-title">
          <div className={styles.heroInner}>
            <p className={styles.eyebrow}>
              <span className={styles.dot} />
              In-memory · Cypher · Rust
            </p>
            <h1 id="features-hero-title" className={styles.title}>
              <span className={styles.titleAccent}>LoraDB</span> features.
            </h1>
            <p className={styles.tagline}>
              An in-memory property graph database with a full Cypher engine,
              written from scratch in Rust. Parser, analyzer, compiler,
              executor and store — small enough to embed, transparent enough
              to read.
            </p>
            <div className={styles.actions}>
              <Link
                to="/docs/getting-started/installation"
                className={clsx(styles.btn, styles.btnPrimary)}
              >
                Get started
                <ArrowIcon />
              </Link>
              <Link
                to="/playground"
                className={clsx(styles.btn, styles.btnSecondary)}
              >
                Try the playground
              </Link>
              <Link
                to="https://github.com/lora-db/lora"
                className={clsx(styles.btn, styles.btnGhost)}
                aria-label="LoraDB on GitHub"
              >
                <svg
                  width="15"
                  height="15"
                  viewBox="0 0 24 24"
                  fill="currentColor"
                  aria-hidden="true"
                >
                  <path d="M12 .5A11.5 11.5 0 0 0 .5 12c0 5.08 3.29 9.39 7.86 10.91.58.11.79-.25.79-.56v-2c-3.2.7-3.87-1.37-3.87-1.37-.52-1.33-1.28-1.69-1.28-1.69-1.05-.72.08-.7.08-.7 1.16.08 1.77 1.2 1.77 1.2 1.03 1.77 2.71 1.26 3.37.96.1-.75.4-1.26.73-1.55-2.55-.29-5.23-1.28-5.23-5.7 0-1.26.45-2.28 1.2-3.09-.12-.29-.52-1.47.11-3.06 0 0 .97-.31 3.18 1.18a11 11 0 0 1 5.79 0c2.21-1.49 3.18-1.18 3.18-1.18.63 1.59.23 2.77.11 3.06.75.81 1.2 1.83 1.2 3.09 0 4.43-2.69 5.41-5.25 5.69.41.35.78 1.04.78 2.1v3.11c0 .31.21.67.8.56A11.5 11.5 0 0 0 23.5 12 11.5 11.5 0 0 0 12 .5z" />
                </svg>
                GitHub
              </Link>
            </div>
            <ul className={styles.heroMeta}>
              <li>
                <span className={styles.heroMetaDot} />
                One Rust engine · seven surfaces
              </li>
              <li>
                <span className={styles.heroMetaDot} />
                Full Cypher pipeline — parse, analyze, compile, execute
              </li>
              <li>
                <span className={styles.heroMetaDot} />
                Local-first · embeddable · open source
              </li>
            </ul>
          </div>
          <div className={styles.heroGlow} aria-hidden="true" />
        </section>

        {/* ---------- FEATURE OVERVIEW ---------- */}
        <section
          className={styles.overview}
          aria-labelledby="features-overview-title"
        >
          <div className={styles.sectionInner}>
            <p className={styles.sectionEyebrow}>What you get</p>
            <h2 id="features-overview-title" className={styles.sectionTitle}>
              A graph engine you can hold in your head.
            </h2>
            <div className={styles.featureGrid}>
              {FEATURE_GROUPS.map((f) => (
                <article key={f.title} className={styles.featureCard}>
                  <div className={styles.featureIcon} aria-hidden="true">
                    <Icon name={f.icon} />
                  </div>
                  <h3>{f.title}</h3>
                  <p>{f.body}</p>
                </article>
              ))}
            </div>
          </div>
        </section>

        {/* ---------- CYPHER COVERAGE ---------- */}
        <section
          className={styles.coverage}
          aria-labelledby="features-coverage-title"
        >
          <div className={styles.sectionInner}>
            <p className={styles.sectionEyebrow}>Cypher coverage</p>
            <h2 id="features-coverage-title" className={styles.sectionTitle}>
              The Cypher you actually write — supported.
            </h2>
            <p className={styles.coverageLede}>
              Pattern matching, writes, aggregation pipelines, paths, temporal
              and spatial predicates — composed top-to-bottom with{' '}
              <CypherCode code="WITH" />.
            </p>
            <div className={styles.coverageGrid}>
              {CYPHER_COVERAGE.map((c) => (
                <article key={c.label} className={styles.coverageItem}>
                  <header className={styles.coverageHeader}>
                    <span className={styles.coverageDot} aria-hidden="true" />
                    <h3>{c.label}</h3>
                  </header>
                  <pre className={styles.coverageCode}>
                    <code>{c.snippet}</code>
                  </pre>
                </article>
              ))}
            </div>
            <p className={styles.coverageFoot}>
              See the{' '}
              <Link to="/docs/queries">queries reference</Link> for the full
              clause list, or browse the{' '}
              <Link to="/docs/functions/overview">functions catalogue</Link>{' '}
              for the 60+ built-ins.
            </p>
          </div>
        </section>

        {/* ---------- SURFACES ---------- */}
        <section
          className={styles.surfaces}
          aria-labelledby="features-surfaces-title"
        >
          <div className={styles.sectionInner}>
            <p className={styles.sectionEyebrow}>Ways to use it</p>
            <h2 id="features-surfaces-title" className={styles.sectionTitle}>
              One engine, seven places to reach it.
            </h2>
            <p className={styles.surfacesLede}>
              Pick the surface that fits the host process. Cypher, parameters,
              and result shapes are identical across all of them.
            </p>
            <div className={styles.surfaceLayout}>
              <div
                className={styles.surfaceList}
                role="tablist"
                aria-label="Runtime surfaces"
              >
                {SURFACES.map((s) => (
                  <button
                    key={s.id}
                    type="button"
                    role="tab"
                    aria-selected={activeSurface === s.id}
                    tabIndex={activeSurface === s.id ? 0 : -1}
                    id={`surface-tab-${s.id}`}
                    aria-controls={`surface-panel-${s.id}`}
                    className={clsx(
                      styles.surfaceItem,
                      activeSurface === s.id && styles.surfaceItemActive,
                    )}
                    onClick={() => setActiveSurface(s.id)}
                  >
                    <span className={styles.surfaceItemIcon} aria-hidden="true">
                      <Icon name={SURFACE_ICONS[s.id]} />
                    </span>
                    <span className={styles.surfaceItemBody}>
                      <span className={styles.surfaceItemLabel}>{s.label}</span>
                      <span className={styles.surfaceItemNote}>{s.note}</span>
                    </span>
                  </button>
                ))}
              </div>

              <div
                className={styles.surfaceCode}
                role="tabpanel"
                id={`surface-panel-${surface.id}`}
                aria-labelledby={`surface-tab-${surface.id}`}
              >
                <div className={styles.codeCard}>
                  <div className={styles.codeCardHeader}>
                    <span className={styles.codeDots} aria-hidden="true">
                      <span />
                      <span />
                      <span />
                    </span>
                    <span className={styles.codeCardLabel}>{surface.note}</span>
                    <span className={styles.codeCardTitle}>{surface.file}</span>
                  </div>
                  <pre className={styles.codeCardBody}>
                    <code>{surface.code}</code>
                  </pre>
                </div>
              </div>
            </div>
          </div>
        </section>

        {/* ---------- LOCAL-FIRST WORKLOADS ---------- */}
        <section
          className={styles.workloads}
          aria-labelledby="features-workloads-title"
        >
          <div className={styles.sectionInner}>
            <p className={styles.sectionEyebrow}>Built for</p>
            <h2 id="features-workloads-title" className={styles.sectionTitle}>
              Local-first graph workloads.
            </h2>
            <div className={styles.workloadGrid}>
              {WORKLOADS.map((w, i) => (
                <article key={w.title} className={styles.workloadCard}>
                  <span className={styles.workloadIndex}>
                    {String(i + 1).padStart(2, '0')}
                  </span>
                  <div>
                    <h3>{w.title}</h3>
                    <p>{w.body}</p>
                  </div>
                </article>
              ))}
            </div>
          </div>
        </section>

        {/* ---------- BOUNDARIES ---------- */}
        <section
          className={styles.boundary}
          aria-labelledby="features-boundary-title"
        >
          <div className={styles.sectionInner}>
            <p className={styles.sectionEyebrow}>Know the boundary</p>
            <h2 id="features-boundary-title" className={styles.sectionTitle}>
              What LoraDB does not pretend to be.
            </h2>
            <p className={styles.boundaryLede}>
              LoraDB is an in-memory engine, not a clustered production
              database. Below is what to expect — stated up front so you can
              decide whether the trade-offs match your workload.
            </p>
            <div className={styles.boundaryGrid}>
              {BOUNDARIES.map((b) => (
                <article key={b.title} className={styles.boundaryCard}>
                  <h3>{b.title}</h3>
                  <p>{b.body}</p>
                </article>
              ))}
            </div>
            <p className={styles.boundaryFoot}>
              Read the full{' '}
              <Link to="/docs/limitations">limitations reference</Link> for an
              exhaustive list of unsupported clauses, operators, and runtime
              features.
            </p>
          </div>
        </section>

        {/* ---------- FINAL CTA ---------- */}
        <section className={styles.cta} aria-labelledby="features-cta-title">
          <div className={styles.sectionInner}>
            <h2 id="features-cta-title" className={styles.ctaTitle}>
              Open a database. Run a query.
            </h2>
            <p className={styles.ctaBody}>
              Three lines of Rust, a curl call, or a single import. The
              fastest way to feel LoraDB is to write a query against it.
            </p>
            <div className={styles.actions}>
              <Link
                to="/docs/getting-started/tutorial"
                className={clsx(styles.btn, styles.btnPrimary)}
              >
                Ten-minute tour
                <ArrowIcon />
              </Link>
              <Link
                to="/docs/queries/examples"
                className={clsx(styles.btn, styles.btnSecondary)}
              >
                Query examples
              </Link>
              <Link
                to="/docs/why"
                className={clsx(styles.btn, styles.btnGhost)}
              >
                Why LoraDB
              </Link>
            </div>
          </div>
        </section>
      </main>
    </Layout>
  );
}
