import React from 'react';
import clsx from 'clsx';
import styles from './styles.module.scss';

// Product mock for the Graph Query Playground. A single 960×560 SVG
// composed as a mini IDE: Cypher query on the left, graph result on
// the right, traffic-light title bar on top.
//
// Node + edge vocabulary intentionally mirrors `BrandGraph` (the
// homepage illustration): core / primary / satellite node kinds,
// `flow` / `soft` edges, quadratic-bezier curves, pulsing core halo.
// Reusing the exact visual language makes the playground feel like
// the same product, just further along.

// Short lines so the editor pane never overflows into the graph
// canvas at any breakpoint. Each line stays under ~32 monospace chars.
const QUERY_LINES = [
  { parts: [['kw', 'MATCH'], ['p', ' (a:'], ['lbl', 'Agent'], ['p', ')']] },
  { parts: [['p', '  -['], ['rel', ':REMEMBERS'], ['p', ']->(c:'], ['lbl', 'Context'], ['p', ')']] },
  { parts: [['p', '  -['], ['rel', ':ABOUT'], ['p', ']->(e:'], ['lbl', 'Entity'], ['p', ')']] },
  { parts: [['kw', 'WHERE'], ['p', ' c.fresh = '], ['kw', 'true']] },
  { parts: [['kw', 'RETURN'], ['p', ' e.id, '], ['fn', 'collect'], ['p', '(c) '], ['kw', 'AS'], ['p', ' ctx']] },
];

// Nodes laid out in a loose hub-and-spoke that matches the query's
// Agent → Context → Entity traversal. Same `kind` vocabulary as
// BrandGraph: core / primary / satellite.
const NODES = [
  { id: 'agent',    x: 720, y: 290, r: 30, label: 'Agent',   kind: 'core' },
  { id: 'c1',       x: 570, y: 160, r: 20, label: 'Context', kind: 'primary' },
  { id: 'c2',       x: 520, y: 290, r: 20, label: 'Context', kind: 'primary' },
  { id: 'c3',       x: 590, y: 430, r: 20, label: 'Context', kind: 'primary' },
  { id: 'e1',       x: 430, y: 120, r: 18, label: 'Entity',  kind: 'primary' },
  { id: 'e2',       x: 410, y: 290, r: 18, label: 'Entity',  kind: 'primary' },
  { id: 'e3',       x: 450, y: 460, r: 18, label: 'Entity',  kind: 'primary' },
  { id: 'tool',     x: 870, y: 175, r: 14, label: 'Tool',    kind: 'satellite' },
  { id: 'session',  x: 880, y: 410, r: 14, label: 'Session', kind: 'satellite' },
];

// Edges: 'flow' = highlighted traversal (brand-gradient, animated
// dash), 'soft' = dashed context edge. Matches BrandGraph's style
// keys so the playground reads as the same illustration family.
const EDGES = [
  ['agent', 'c1', 'flow'],
  ['agent', 'c2', 'flow'],
  ['agent', 'c3', 'flow'],
  ['c1', 'e1', 'flow'],
  ['c2', 'e2', 'flow'],
  ['c3', 'e3', 'flow'],
  ['agent', 'tool', 'soft'],
  ['agent', 'session', 'soft'],
];

// Quadratic-bezier between two nodes with a perpendicular bend —
// same helper (and feel) as BrandGraph's `edgePath`.
function edgePath(a, b, bend = 0.08) {
  const mx = (a.x + b.x) / 2;
  const my = (a.y + b.y) / 2;
  const dx = b.x - a.x;
  const dy = b.y - a.y;
  const cx = mx + -dy * bend;
  const cy = my + dx * bend;
  return `M ${a.x} ${a.y} Q ${cx} ${cy} ${b.x} ${b.y}`;
}

function Part({ kind, children, i }) {
  return (
    <tspan key={i} className={styles[`tok_${kind}`]}>
      {children}
    </tspan>
  );
}

export default function PlaygroundPreview() {
  const byId = Object.fromEntries(NODES.map((n) => [n.id, n]));

  return (
    <div className={styles.frame} role="img" aria-label="Preview of the LoraDB Graph Query Playground">
      {/* --- window chrome --- */}
      <div className={styles.chrome}>
        <span className={styles.dots} aria-hidden="true">
          <span />
          <span />
          <span />
        </span>
        <span className={styles.address}>play.loradb.com</span>
        <span className={styles.runChip} aria-hidden="true">
          <svg width="10" height="10" viewBox="0 0 24 24" fill="currentColor" aria-hidden="true">
            <path d="M8 5v14l11-7z" />
          </svg>
          Run
        </span>
      </div>

      <div className={styles.body}>
        <svg
          viewBox="0 0 960 560"
          xmlns="http://www.w3.org/2000/svg"
          className={styles.svg}
          role="presentation"
          aria-hidden="true"
          preserveAspectRatio="xMidYMid meet"
        >
          <defs>
            {/* Same shape as BrandGraph's linear gradient — diagonal
                across each path's object bounding box so curved paths
                get a visible blue→violet gradient reliably. */}
            <linearGradient id="pgEdge" x1="0%" y1="0%" x2="100%" y2="100%">
              <stop offset="0%" stopColor="var(--brand-accent-a)" />
              <stop offset="100%" stopColor="var(--brand-accent-b)" />
            </linearGradient>

            <linearGradient id="pgCore" x1="0%" y1="0%" x2="100%" y2="100%">
              <stop offset="0%" stopColor="var(--brand-accent-a)" />
              <stop offset="100%" stopColor="var(--brand-accent-b)" />
            </linearGradient>

            <radialGradient id="pgCoreGlow" cx="50%" cy="50%" r="50%">
              <stop offset="0%" stopColor="var(--brand-graph-glow-inner)" stopOpacity="1" />
              <stop offset="60%" stopColor="var(--brand-graph-glow-mid)" stopOpacity="0.45" />
              <stop offset="100%" stopColor="var(--brand-graph-glow-outer)" stopOpacity="0" />
            </radialGradient>

            <filter id="pgBlur" x="-50%" y="-50%" width="200%" height="200%">
              <feGaussianBlur stdDeviation="18" />
            </filter>
          </defs>

          {/* subtle split divider between panes */}
          <line
            x1="380"
            y1="40"
            x2="380"
            y2="520"
            className={styles.divider}
          />

          {/* ---------- LEFT PANE: query editor ---------- */}
          <g className={styles.editor}>
            {/* gutter */}
            <rect x="20" y="40" width="38" height="480" rx="8" className={styles.gutter} />
            {QUERY_LINES.map((line, i) => (
              <React.Fragment key={i}>
                <text
                  x="48"
                  y={92 + i * 38}
                  className={styles.lineno}
                  textAnchor="end"
                  fontFamily="var(--ifm-font-family-monospace)"
                  fontSize="13"
                >
                  {i + 1}
                </text>
                <text
                  x="68"
                  y={92 + i * 38}
                  className={styles.codeLine}
                  fontFamily="var(--ifm-font-family-monospace)"
                  fontSize="16"
                >
                  {line.parts.map((p, j) => (
                    <Part key={j} kind={p[0]} i={j}>
                      {p[1]}
                    </Part>
                  ))}
                </text>
              </React.Fragment>
            ))}

            {/* caret — suggests an editable cursor at the end of
                the RETURN line */}
            <rect
              className={styles.caret}
              x="360"
              y={92 + 4 * 38 - 14}
              width="2"
              height="18"
            />

            {/* result count pill bottom-left */}
            <g transform="translate(80 485)">
              <rect width="225" height="30" rx="15" className={styles.pill} />
              <circle cx="16" cy="15" r="3.5" className={styles.pillDot} />
              <text
                x="30"
                y="19"
                className={styles.pillText}
                fontFamily="var(--ifm-font-family-monospace)"
                fontSize="12"
              >
                3 paths · 9 rows · 18 ms
              </text>
            </g>
          </g>

          {/* ---------- RIGHT PANE: graph canvas ---------- */}
          <g className={styles.canvas}>
            {/* Ambient glow behind the core node — same pattern as
                BrandGraph's `.glow`. */}
            <circle
              cx={byId.agent.x}
              cy={byId.agent.y}
              r="170"
              fill="url(#pgCoreGlow)"
              className={styles.glow}
            />

            {/* Edges first so nodes sit on top. Alternating bend
                sign keeps crossings from bowing the same way. */}
            <g className={styles.edges}>
              {EDGES.map(([fromId, toId, variant], i) => {
                const a = byId[fromId];
                const b = byId[toId];
                const bend = ((i % 2) === 0 ? 1 : -1) * 0.08;
                const d = edgePath(a, b, bend);
                return (
                  <path
                    key={`${fromId}-${toId}`}
                    d={d}
                    className={clsx(
                      styles.edge,
                      variant === 'flow' && styles.edgeFlow,
                      variant === 'soft' && styles.edgeSoft,
                    )}
                    style={{ '--edge-delay': `${(i % 4) * 0.5}s` }}
                  />
                );
              })}
            </g>

            {/* Nodes — same layered structure as BrandGraph:
                halo (core), ring, dot, small inner core, label. */}
            <g className={styles.nodes}>
              {NODES.map((n, i) => (
                <g key={n.id} transform={`translate(${n.x} ${n.y})`}>
                  <g
                    className={clsx(styles.node, styles[`node_${n.kind}`])}
                    style={{ '--node-delay': `${i * 0.25}s` }}
                  >
                    {n.kind === 'core' && (
                      <circle r={n.r + 14} className={styles.nodeHalo} />
                    )}
                    {n.kind !== 'satellite' && (
                      <circle r={n.r + 6} className={styles.nodeRing} />
                    )}
                    <circle
                      r={n.r}
                      className={styles.nodeDot}
                      fill={n.kind === 'core' ? 'url(#pgCore)' : undefined}
                    />
                    <circle r={n.r * 0.35} className={styles.nodeCore} />
                    {n.kind !== 'satellite' && (
                      <text
                        y={n.r + 20}
                        className={styles.nodeLabel}
                        textAnchor="middle"
                      >
                        {n.label}
                      </text>
                    )}
                    {n.kind === 'satellite' && (
                      <text
                        y={n.r + 16}
                        className={clsx(styles.nodeLabel, styles.nodeLabelSmall)}
                        textAnchor="middle"
                      >
                        {n.label}
                      </text>
                    )}
                  </g>
                </g>
              ))}
            </g>

            {/* floating legend top-right */}
            <g transform="translate(730 60)" className={styles.legend}>
              <rect width="210" height="66" rx="10" className={styles.legendBg} />
              <g transform="translate(16 22)">
                <line
                  x1="0"
                  y1="0"
                  x2="24"
                  y2="0"
                  stroke="url(#pgEdge)"
                  strokeWidth="2"
                  strokeDasharray="4 4"
                  strokeLinecap="round"
                />
                <text x="34" y="4" className={styles.legendText} fontSize="11">
                  Highlighted path
                </text>
              </g>
              <g transform="translate(16 46)">
                <line
                  x1="0"
                  y1="0"
                  x2="24"
                  y2="0"
                  className={styles.legendEdgeSoft}
                  strokeWidth="1.5"
                  strokeLinecap="round"
                />
                <text x="34" y="4" className={styles.legendText} fontSize="11">
                  Context edge
                </text>
              </g>
            </g>
          </g>
        </svg>
      </div>
    </div>
  );
}
