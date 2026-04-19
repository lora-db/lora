import React from 'react';
import clsx from 'clsx';
import styles from './styles.module.scss';

// A composed graph of labeled nodes and typed edges, used as the
// signature homepage illustration. Pure SVG — no runtime deps, no
// canvas, no images. Coordinates live in a 800x600 viewBox and scale
// fluidly. Animation is layered on via CSS and respects
// `prefers-reduced-motion`.
//
// The nodes aren't arbitrary: the labels and edges deliberately evoke
// the shape of an agent / memory / event system, which is what LoraDB
// is built to live inside.

const NODES = [
  { id: 'agent',    x: 400, y: 312, r: 30, label: 'Agent',       kind: 'core' },
  { id: 'memory',   x: 200, y: 220, r: 22, label: 'Memory',      kind: 'primary' },
  { id: 'tool',     x: 600, y: 200, r: 22, label: 'Tool',        kind: 'primary' },
  { id: 'event',    x: 170, y: 410, r: 20, label: 'Event',       kind: 'primary' },
  { id: 'entity',   x: 400, y: 488, r: 22, label: 'Entity',      kind: 'primary' },
  { id: 'obs',      x: 630, y: 402, r: 20, label: 'Observation', kind: 'primary' },
  { id: 'decision', x: 400, y: 132, r: 20, label: 'Decision',    kind: 'primary' },
  { id: 'session',  x: 70,  y: 112, r: 14, label: 'Session',     kind: 'satellite' },
  { id: 'scene',    x: 92,  y: 520, r: 14, label: 'Scene',       kind: 'satellite' },
  { id: 'plan',     x: 730, y: 108, r: 14, label: 'Plan',        kind: 'satellite' },
  { id: 'signal',   x: 720, y: 520, r: 14, label: 'Signal',      kind: 'satellite' },
];

// edges: [from, to, style?]. `style` keys: 'flow' = animated dash,
// 'soft' = static faint, default = static.
const EDGES = [
  ['agent',    'memory',   'flow'],
  ['agent',    'tool',     'flow'],
  ['agent',    'event'],
  ['agent',    'entity',   'flow'],
  ['agent',    'obs'],
  ['agent',    'decision', 'flow'],
  ['memory',   'entity'],
  ['memory',   'decision', 'soft'],
  ['tool',     'decision'],
  ['tool',     'obs',      'soft'],
  ['event',    'entity'],
  ['event',    'obs',      'soft'],
  ['obs',      'entity'],
  ['memory',   'session',  'soft'],
  ['event',    'scene',    'soft'],
  ['tool',     'plan',     'soft'],
  ['obs',      'signal',   'soft'],
];

const byId = Object.fromEntries(NODES.map((n) => [n.id, n]));

// Build a quadratic-bezier path between two nodes with a subtle
// curvature perpendicular to the midpoint. Keeps the composition from
// feeling like a wire diagram.
function edgePath(a, b, bend = 0.08) {
  const mx = (a.x + b.x) / 2;
  const my = (a.y + b.y) / 2;
  const dx = b.x - a.x;
  const dy = b.y - a.y;
  // perpendicular offset — sign alternates so crossings don't all bow
  // in the same direction.
  const cx = mx + -dy * bend;
  const cy = my + dx * bend;
  return `M ${a.x} ${a.y} Q ${cx} ${cy} ${b.x} ${b.y}`;
}

export default function BrandGraph({ className }) {
  return (
    <div className={clsx(styles.wrap, className)} aria-hidden="true">
      <svg
        className={styles.svg}
        viewBox="0 0 800 600"
        preserveAspectRatio="xMidYMid meet"
        role="img"
        focusable="false"
      >
        <defs>
          <radialGradient id="brandGraphCoreGlow" cx="50%" cy="50%" r="50%">
            <stop offset="0%" stopColor="var(--brand-graph-glow-inner)" stopOpacity="1" />
            <stop offset="60%" stopColor="var(--brand-graph-glow-mid)" stopOpacity="0.45" />
            <stop offset="100%" stopColor="var(--brand-graph-glow-outer)" stopOpacity="0" />
          </radialGradient>

          <linearGradient id="brandGraphEdge" x1="0%" y1="0%" x2="100%" y2="100%">
            <stop offset="0%" stopColor="var(--brand-accent-a)" />
            <stop offset="100%" stopColor="var(--brand-accent-b)" />
          </linearGradient>

          <linearGradient id="brandGraphCore" x1="0%" y1="0%" x2="100%" y2="100%">
            <stop offset="0%" stopColor="var(--brand-accent-a)" />
            <stop offset="100%" stopColor="var(--brand-accent-b)" />
          </linearGradient>

          <filter id="brandGraphBlur" x="-50%" y="-50%" width="200%" height="200%">
            <feGaussianBlur stdDeviation="18" />
          </filter>
        </defs>

        {/* Ambient glow behind the core node. */}
        <circle
          cx={byId.agent.x}
          cy={byId.agent.y}
          r={220}
          fill="url(#brandGraphCoreGlow)"
          className={styles.glow}
        />

        {/* Edges first so nodes sit on top. */}
        <g className={styles.edges}>
          {EDGES.map(([fromId, toId, variant], i) => {
            const a = byId[fromId];
            const b = byId[toId];
            // Alternate bend sign for visual rhythm.
            const bend = ((i % 2) === 0 ? 1 : -1) * 0.07;
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
                style={{ '--edge-delay': `${(i % 6) * 0.6}s` }}
              />
            );
          })}
        </g>

        <g className={styles.nodes}>
          {NODES.map((n, i) => (
            // Two groups: the outer one carries the static SVG
            // transform so nodes stay at their coordinates; the inner
            // one carries the CSS animation so it can't clobber the
            // translate.
            <g key={n.id} transform={`translate(${n.x} ${n.y})`}>
              <g
                className={clsx(
                  styles.node,
                  styles[`node_${n.kind}`],
                )}
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
                  fill={n.kind === 'core' ? 'url(#brandGraphCore)' : undefined}
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
      </svg>
    </div>
  );
}