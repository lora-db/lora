import React from 'react';
import clsx from 'clsx';
import Layout from '@theme/Layout';
import Link from '@docusaurus/Link';

import CypherCode from '@site/src/components/CypherCode';
import LinkCard from '@site/src/components/LinkCard';
import styles from './features.module.scss';

// -------------------------------------------------------------------
// Static content
// -------------------------------------------------------------------

// Anchor strip below the hero. Each entry maps to a section id below.
const ANCHORS = [
  { id: 'principles', label: 'Principles' },
  { id: 'coverage', label: 'Cypher coverage' },
  { id: 'functions', label: 'Functions & types' },
  { id: 'architecture', label: 'Architecture' },
  { id: 'surfaces', label: 'Surfaces' },
  { id: 'operations', label: 'Operations' },
  { id: 'limits', label: 'Limits' },
];

// Design principles — moved from the homepage's "value props" block
// so the features page leads with the bets the engine made before
// listing what it supports.
const PRINCIPLES = [
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

// Each Cypher coverage block uses CypherCode for the snippet so the
// tokens render with the same colours as fenced code blocks elsewhere
// on the site. Each card now carries an explicit "→ reference" route
// so the page is a router, not just a brochure.
const CYPHER_COVERAGE = [
  {
    label: 'Pattern matching',
    snippet:
      "MATCH (a:Person)-[:KNOWS]->(b:Person)\nWHERE a.city = 'Berlin'\nRETURN a.name, collect(b.name) AS friends",
    to: '/docs/queries/match',
    linkLabel: 'MATCH reference',
  },
  {
    label: 'Writing data',
    snippet:
      "MERGE (u:User {email: $email})\nON CREATE SET u.created = datetime()\nON MATCH  SET u.last_seen = datetime()",
    to: '/docs/queries/unwind-merge',
    linkLabel: 'MERGE reference',
  },
  {
    label: 'Variable-length paths',
    snippet:
      'MATCH p = shortestPath(\n  (a:Stop {code: $from})-[:CONNECTS*..6]->(b:Stop {code: $to})\n)\nRETURN length(p) AS hops, [n IN nodes(p) | n.code] AS via',
    to: '/docs/queries/paths',
    linkLabel: 'Paths reference',
  },
  {
    label: 'Aggregation pipelines',
    snippet:
      "MATCH (u:User)-[:PLACED]->(o:Order {status: 'paid'})\nWITH u, count(o) AS orders, sum(o.total) AS spend\nWHERE orders >= 3\nRETURN u.email, orders, spend ORDER BY spend DESC",
    to: '/docs/queries/aggregation',
    linkLabel: 'Aggregation reference',
  },
  {
    label: 'Temporal predicates',
    snippet:
      "MATCH (e:Event)\nWHERE e.at >= datetime() - duration('P7D')\nRETURN date(e.at) AS day, count(*) AS events\nORDER BY day",
    to: '/docs/data-types/temporal',
    linkLabel: 'Temporal types',
  },
  {
    label: 'Spatial distance',
    snippet:
      "WITH point({latitude: 52.52, longitude: 13.405}) AS origin\nMATCH (s:Store)\nWHERE distance(s.loc, origin) < 5000\nRETURN s.name, distance(s.loc, origin) AS metres\nORDER BY metres",
    to: '/docs/data-types/spatial',
    linkLabel: 'Spatial types',
  },
];

// Function categories. Mirrors the categories table in
// docs/functions/overview.md so links resolve cleanly.
const FUNCTION_CATEGORIES = [
  { label: 'Aggregation', to: '/docs/functions/aggregation' },
  { label: 'String', to: '/docs/functions/string' },
  { label: 'Math', to: '/docs/functions/math' },
  { label: 'List', to: '/docs/functions/list' },
  { label: 'Temporal', to: '/docs/functions/temporal' },
  { label: 'Spatial', to: '/docs/functions/spatial' },
  { label: 'Vector', to: '/docs/functions/vectors' },
];

const TYPE_CATEGORIES = [
  { label: 'Scalars', to: '/docs/data-types/scalars' },
  { label: 'Lists & Maps', to: '/docs/data-types/lists-and-maps' },
  { label: 'Temporal', to: '/docs/data-types/temporal' },
  { label: 'Spatial', to: '/docs/data-types/spatial' },
  { label: 'Vectors', to: '/docs/data-types/vectors' },
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

// Result-format chips shown alongside the architecture pipeline so
// readers see that "format" is a per-query knob, not a binding-level
// decision.
const RESULT_FORMATS = ['rows', 'rowArrays', 'graph', 'combined'];

const SURFACES = [
  {
    id: 'rust',
    label: 'Rust crate',
    file: 'main.rs',
    note: 'lora-database',
    guideTo: '/docs/getting-started/rust',
    guideLabel: 'Rust guide',
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
    guideTo: '/docs/getting-started/server',
    guideLabel: 'Server guide',
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
    guideTo: '/docs/getting-started/node',
    guideLabel: 'Node.js guide',
    code: `import { createDatabase } from '@loradb/lora-node';

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
    guideTo: '/docs/getting-started/python',
    guideLabel: 'Python guide',
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
    guideTo: '/docs/getting-started/wasm',
    guideLabel: 'WASM guide',
    code: `import { createDatabase } from '@loradb/lora-wasm';

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
    guideTo: '/docs/getting-started/go',
    guideLabel: 'Go guide',
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
    guideTo: '/docs/getting-started/ruby',
    guideLabel: 'Ruby guide',
    code: `require "lora_ruby"

db = LoraRuby::Database.create
db.execute("CREATE (:User {name: 'Ada'})")

result = db.execute(
  "MATCH (u:User) RETURN u.name"
)`,
  },
];

const BOUNDARIES = [
  {
    title: 'In-memory only',
    body: 'The store is BTreeMap-backed and lives in process memory. Manual snapshots exist; continuous durability does not.',
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

// Architecture pipeline arrow drawn between stages.
function StageArrow() {
  return (
    <svg
      className={styles.archArrow}
      width="22"
      height="14"
      viewBox="0 0 24 14"
      fill="none"
      stroke="currentColor"
      strokeWidth="1.6"
      strokeLinecap="round"
      strokeLinejoin="round"
      aria-hidden="true"
    >
      <path d="M2 7h18M14 1l6 6-6 6" />
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

  // Scroll-spy for the "On this page" strip. The active section is the
  // last one whose top has crossed the nav line (NAV_OFFSET pixels from
  // the viewport top). This matches reader intuition: "I'm parked in
  // whichever section last scrolled past the bar."
  //
  // We also mirror the Docusaurus navbar's hideOnScroll state so the
  // strip can slide up with the navbar (instead of leaving an empty
  // band above itself when the navbar is hidden).
  const [activeAnchor, setActiveAnchor] = React.useState(ANCHORS[0]?.id);
  const [navbarHidden, setNavbarHidden] = React.useState(false);
  const anchorNavRef = React.useRef(null);
  React.useEffect(() => {
    const els = ANCHORS.map((a) => document.getElementById(a.id)).filter(
      Boolean,
    );
    if (els.length === 0) return undefined;

    // Active threshold = navbar height + anchor-strip height + small
    // buffer. A section becomes active once its top has crossed that
    // line — i.e. once it sits visually under the strip.
    const NAV_OFFSET = 130;
    let frame = 0;

    // Walk offsetParents to get the strip's absolute document Y.
    // Layout-based, transform-independent — safe to call while the
    // strip is currently lifted.
    const getAbsTop = (el) => {
      let y = 0;
      let cur = el;
      while (cur) {
        y += cur.offsetTop;
        cur = cur.offsetParent;
      }
      return y;
    };

    // Track scroll direction so the strip's hide/reveal animation
    // triggers at the *same* scroll event as the Docusaurus navbar's,
    // not midway through the navbar's transition.
    let lastY = window.scrollY;
    let lifted = false;
    const TOP_REVEAL_THRESHOLD = 60; // matches Docusaurus's "always show near top"

    const update = () => {
      frame = 0;
      let current = els[0].id;
      for (const el of els) {
        if (el.getBoundingClientRect().top - NAV_OFFSET <= 0) current = el.id;
      }
      setActiveAnchor(current);

      const navbar = document.querySelector('.navbar');
      const strip = anchorNavRef.current;
      if (!navbar || !strip) {
        setNavbarHidden(false);
        return;
      }

      const y = window.scrollY;
      const navHeight = navbar.offsetHeight || 60;
      const stripIsPinned = y >= getAbsTop(strip) - navHeight;
      const goingUp = y < lastY;
      const goingDown = y > lastY;

      // Reveal: scrolling up, or near the top of the page. Hide: only
      // once the strip is sticky-pinned AND the user is scrolling
      // down. Below the pin threshold, the strip lives inside the
      // hero in normal flow — never lift it then.
      if (goingUp || y < TOP_REVEAL_THRESHOLD) {
        lifted = false;
      } else if (goingDown && stripIsPinned) {
        lifted = true;
      }
      setNavbarHidden(lifted);
      lastY = y;
    };

    const onScroll = () => {
      if (frame) return;
      frame = window.requestAnimationFrame(update);
    };

    update();
    window.addEventListener('scroll', onScroll, { passive: true });
    window.addEventListener('resize', onScroll);
    return () => {
      if (frame) cancelAnimationFrame(frame);
      window.removeEventListener('scroll', onScroll);
      window.removeEventListener('resize', onScroll);
    };
  }, []);

  return (
    <Layout
      title="Features"
      description="LoraDB is an in-memory graph database with a full Cypher engine, written in Rust. Explore the query pipeline, language coverage, runtime surfaces, and the lines we won't pretend to cross."
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
              Everything <span className={styles.titleAccent}>LoraDB</span>{' '}
              supports — and what it doesn’t.
            </h1>
            <p className={styles.tagline}>
              A complete map of the engine — Cypher coverage, surfaces,
              architecture, and the lines we won’t pretend to cross. Pick what
              you came here to verify.
            </p>
            <div className={styles.actions}>
              <Link
                to="/docs/queries/cheat-sheet"
                className={clsx(styles.btn, styles.btnPrimary)}
              >
                Cheat sheet
                <ArrowIcon />
              </Link>
              <Link
                to="/docs/limitations"
                className={clsx(styles.btn, styles.btnSecondary)}
              >
                Limitations
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

        {/* ---------- ANCHOR NAV ---------- */}
        <nav
          ref={anchorNavRef}
          className={clsx(
            styles.anchorNav,
            navbarHidden && styles.anchorNavLifted,
          )}
          aria-label="On this page"
        >
          <div className={styles.anchorNavInner}>
            <span className={styles.anchorNavLabel}>On this page</span>
            <ul className={styles.anchorNavList}>
              {ANCHORS.map((a) => (
                <li key={a.id}>
                  <a
                    href={`#${a.id}`}
                    className={clsx(
                      styles.anchorNavLink,
                      activeAnchor === a.id && styles.anchorNavLinkActive,
                    )}
                    aria-current={activeAnchor === a.id ? 'true' : undefined}
                  >
                    {a.label}
                  </a>
                </li>
              ))}
            </ul>
          </div>
        </nav>

        {/* ---------- DESIGN PRINCIPLES ---------- */}
        <section
          id="principles"
          className={styles.principles}
          aria-labelledby="features-principles-title"
        >
          <div className={styles.sectionInner}>
            <p className={styles.sectionEyebrow}>Design principles</p>
            <h2
              id="features-principles-title"
              className={styles.sectionTitle}
            >
              The bets the engine took.
            </h2>
            <div className={styles.principlesGrid}>
              {PRINCIPLES.map((p, i) => (
                <article key={p.title} className={styles.principleCard}>
                  <span className={styles.principleIndex}>
                    {String(i + 1).padStart(2, '0')}
                  </span>
                  <div>
                    <h3>{p.title}</h3>
                    <p>{p.body}</p>
                  </div>
                </article>
              ))}
            </div>
          </div>
        </section>

        {/* ---------- CYPHER COVERAGE ---------- */}
        <section
          id="coverage"
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
                  <Link to={c.to} className={styles.coverageLink}>
                    {c.linkLabel}
                    <ArrowIcon />
                  </Link>
                </article>
              ))}
            </div>
            <div className={styles.coverageFooter}>
              <LinkCard
                to="/docs/queries/cheat-sheet"
                eyebrow="One-pager"
                title="Cypher cheat sheet"
                variant="compact"
              />
              <LinkCard
                to="/docs/queries/examples"
                eyebrow="Tour"
                title="Copy-paste examples"
                variant="compact"
              />
              <LinkCard
                to="/docs/queries"
                eyebrow="Reference"
                title="Every clause, indexed"
                variant="compact"
              />
            </div>
          </div>
        </section>

        {/* ---------- FUNCTIONS & DATA TYPES ---------- */}
        <section
          id="functions"
          className={styles.functions}
          aria-labelledby="features-functions-title"
        >
          <div className={styles.sectionInner}>
            <p className={styles.sectionEyebrow}>Functions & types</p>
            <h2 id="features-functions-title" className={styles.sectionTitle}>
              60+ built-ins. Every value typed.
            </h2>
            <div className={styles.functionsGrid}>
              <article className={styles.catalogue}>
                <header className={styles.catalogueHeader}>
                  <h3>Functions</h3>
                  <p>
                    String, math, list, aggregation, temporal, spatial, vector
                    — shipped with the engine. No procedure plugins to install.
                  </p>
                </header>
                <ul className={styles.chipList}>
                  {FUNCTION_CATEGORIES.map((c) => (
                    <li key={c.label}>
                      <Link to={c.to} className={styles.chip}>
                        {c.label}
                      </Link>
                    </li>
                  ))}
                </ul>
                <Link
                  to="/docs/functions/overview"
                  className={styles.catalogueLink}
                >
                  All functions
                  <ArrowIcon />
                </Link>
              </article>

              <article className={styles.catalogue}>
                <header className={styles.catalogueHeader}>
                  <h3>Data types</h3>
                  <p>
                    Every value — stored as a property, projected in a RETURN,
                    or bound as a parameter — has one of these types.
                  </p>
                </header>
                <ul className={styles.chipList}>
                  {TYPE_CATEGORIES.map((c) => (
                    <li key={c.label}>
                      <Link to={c.to} className={styles.chip}>
                        {c.label}
                      </Link>
                    </li>
                  ))}
                </ul>
                <Link
                  to="/docs/data-types/overview"
                  className={styles.catalogueLink}
                >
                  All data types
                  <ArrowIcon />
                </Link>
              </article>
            </div>
          </div>
        </section>

        {/* ---------- ARCHITECTURE (NEW — renders the pipeline) ---------- */}
        <section
          id="architecture"
          className={styles.architecture}
          aria-labelledby="features-architecture-title"
        >
          <div className={styles.sectionInner}>
            <p className={styles.sectionEyebrow}>Architecture</p>
            <h2
              id="features-architecture-title"
              className={styles.sectionTitle}
            >
              A compiler-style pipeline you can read.
            </h2>
            <p className={styles.architectureLede}>
              Every query walks a real pipeline — parser, analyzer, compiler,
              executor — written from scratch in Rust. Each stage is one crate;
              the names line up with the source tree.
            </p>
            <ol className={styles.archStages}>
              {PIPELINE_STAGES.map((s, i) => (
                <React.Fragment key={s.name}>
                  <li className={styles.archStage}>
                    <span className={styles.archStep}>{s.step}</span>
                    <h3 className={styles.archStageName}>{s.name}</h3>
                    <code className={styles.archCrate}>{s.crate}</code>
                    <p className={styles.archStageBody}>{s.body}</p>
                  </li>
                  {i < PIPELINE_STAGES.length - 1 ? (
                    <li
                      className={styles.archConnector}
                      aria-hidden="true"
                    >
                      <StageArrow />
                    </li>
                  ) : null}
                </React.Fragment>
              ))}
            </ol>

            <div className={styles.archFooter}>
              <article className={styles.archFormats}>
                <header>
                  <p className={styles.miniEyebrow}>Result formats</p>
                  <h3>Pick a shape per query</h3>
                </header>
                <ul className={styles.formatChips}>
                  {RESULT_FORMATS.map((f) => (
                    <li key={f}>
                      <code>{f}</code>
                    </li>
                  ))}
                </ul>
                <Link
                  to="/docs/concepts/result-formats"
                  className={styles.catalogueLink}
                >
                  Result formats reference
                  <ArrowIcon />
                </Link>
              </article>
              <Link
                to="https://github.com/lora-db/lora"
                className={styles.archSourceLink}
              >
                <span>
                  <span className={styles.miniEyebrow}>Open the source</span>
                  Read the engine on GitHub
                </span>
                <ArrowIcon />
              </Link>
            </div>
          </div>
        </section>

        {/* ---------- SURFACES ---------- */}
        <section
          id="surfaces"
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
                <Link to={surface.guideTo} className={styles.surfaceGuideLink}>
                  {surface.guideLabel}
                  <ArrowIcon />
                </Link>
              </div>
            </div>
          </div>
        </section>

        {/* ---------- OPERATIONS — HTTP & SNAPSHOTS ---------- */}
        <section
          id="operations"
          className={styles.operations}
          aria-labelledby="features-operations-title"
        >
          <div className={styles.sectionInner}>
            <p className={styles.sectionEyebrow}>Operations</p>
            <h2
              id="features-operations-title"
              className={styles.sectionTitle}
            >
              Run it as a server. Save it to a file.
            </h2>
            <div className={styles.opsGrid}>
              <LinkCard
                to="/docs/api/http"
                eyebrow="HTTP API"
                title="Run LoraDB as a server"
              >
                One Axum process serves exactly one in-memory graph. Health,
                query, and opt-in admin endpoints — meant to live behind your
                own ingress.
              </LinkCard>
              <LinkCard
                to="/docs/snapshot"
                eyebrow="Snapshots"
                title="Manual point-in-time saves"
              >
                Dump the full graph to a single file and load it back later.
                Operator-controlled, atomic on rename — not a WAL, not
                continuous durability.
              </LinkCard>
            </div>
          </div>
        </section>

        {/* ---------- BOUNDARIES ---------- */}
        <section
          id="limits"
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
            <div className={styles.boundaryFooter}>
              <LinkCard
                to="/docs/limitations"
                eyebrow="Reference"
                title="The full limitations list"
                variant="accent"
              >
                Every unsupported clause, operator, function, and runtime
                feature — with what to reach for instead.
              </LinkCard>
              <LinkCard
                to="/docs/troubleshooting"
                eyebrow="Errors"
                title="Troubleshooting"
                variant="compact"
              />
            </div>
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
                to="/docs/queries/cheat-sheet"
                className={clsx(styles.btn, styles.btnSecondary)}
              >
                Cheat sheet
              </Link>
            </div>
          </div>
        </section>
      </main>
    </Layout>
  );
}
