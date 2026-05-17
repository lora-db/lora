import { useEffect, useMemo, useRef, useState } from "react";
import type { Meta, StoryObj } from "@storybook/react";
import {
  LoraGraphCanvas,
  type LoraGraphCanvasHandle,
  type GraphData,
} from ".";
import { darkTheme } from "./theme/presets";

const SMALL_GRAPH: GraphData = {
  nodes: [
    { id: "alice", group: "person" },
    { id: "bob", group: "person" },
    { id: "carol", group: "person" },
    { id: "loradb", group: "company" },
    { id: "acme", group: "company" },
  ],
  links: [
    { source: "alice", target: "bob" },
    { source: "bob", target: "carol" },
    { source: "alice", target: "loradb" },
    { source: "carol", target: "acme" },
  ],
};

const meta: Meta<typeof LoraGraphCanvas> = {
  title: "LoraGraphCanvas",
  component: LoraGraphCanvas,
  parameters: {
    layout: "fullscreen",
  },
  tags: ["autodocs"],
};

export default meta;
type Story = StoryObj<typeof LoraGraphCanvas>;

// 1. Basic 2D
export const Basic: Story = {
  render: () => (
    <div style={{ width: "100vw", height: "100vh" }}>
      <LoraGraphCanvas
        defaultData={SMALL_GRAPH}
        nodeLabel="id"
        nodeAutoColorBy="group"
        backgroundColor="#fafbfc"
      />
    </div>
  ),
};

// 2. 3D mode
export const ThreeDimensional: Story = {
  render: () => (
    <div style={{ width: "100vw", height: "100vh" }}>
      <LoraGraphCanvas
        defaultMode="3d"
        defaultData={SMALL_GRAPH}
        nodeLabel="id"
        nodeAutoColorBy="group"
        backgroundColor="#0e1014"
        theme={darkTheme}
      />
    </div>
  ),
};

// 3. Mode toggle — same data instance survives the switch
export const ModeToggle: Story = {
  render: () => (
    <div style={{ width: "100vw", height: "100vh" }}>
      <LoraGraphCanvas
        defaultMode="2d"
        defaultData={SMALL_GRAPH}
        nodeLabel="id"
        nodeAutoColorBy="group"
      />
    </div>
  ),
  parameters: {
    docs: {
      description: {
        story:
          "Use the cube button in the toolbar (or the `3` key) to toggle between 2D and 3D. The data is preserved across the switch.",
      },
    },
  },
};

// 4. Build-a-graph — empty start, all tools enabled
function BuildAGraph() {
  return (
    <div style={{ width: "100vw", height: "100vh" }}>
      <LoraGraphCanvas
        defaultData={{ nodes: [], links: [] }}
        nodeLabel="id"
        nodeAutoColorBy="group"
        backgroundColor="#fafbfc"
      />
      <div
        style={{
          position: "fixed",
          left: 12,
          bottom: 12,
          padding: "8px 12px",
          background: "rgba(255,255,255,0.92)",
          borderRadius: 6,
          fontSize: 12,
          maxWidth: 320,
          lineHeight: 1.4,
          border: "1px solid #d8dde3",
          fontFamily: "system-ui, sans-serif",
        }}
      >
        <b>How to use:</b>
        <ol style={{ margin: "4px 0 0 16px", padding: 0 }}>
          <li>Click the “add node” button (or press <code>N</code>).</li>
          <li>Click anywhere on the canvas to drop a node.</li>
          <li>Press <code>L</code>, then click two nodes to link them.</li>
          <li>Select a node and press <code>⌫</code> to delete it.</li>
          <li>Right-click a node or the canvas for more actions.</li>
        </ol>
      </div>
    </div>
  );
}
export const BuildAGraphStory: Story = { render: () => <BuildAGraph /> };
BuildAGraphStory.storyName = "Build-a-graph (empty)";

// 5. Headless (no toolbar, host-controlled)
function Headless() {
  const ref = useRef<LoraGraphCanvasHandle>(null);
  return (
    <div style={{ width: "100vw", height: "100vh", position: "relative" }}>
      <LoraGraphCanvas
        ref={ref}
        tools={false}
        defaultData={SMALL_GRAPH}
        nodeLabel="id"
        nodeAutoColorBy="group"
      />
      <div
        style={{
          position: "fixed",
          left: 12,
          top: 12,
          display: "flex",
          gap: 6,
          fontFamily: "system-ui, sans-serif",
          fontSize: 12,
        }}
      >
        <button onClick={() => ref.current?.addNode()}>+ node</button>
        <button
          onClick={() => {
            const ids = ref.current?.getData().nodes.slice(-2).map((n) => n.id);
            if (ids && ids.length === 2) ref.current?.addLink({
              source: ids[0]!,
              target: ids[1]!,
            });
          }}
        >
          + link (last two)
        </button>
        <button onClick={() => ref.current?.fit(400, 40)}>fit</button>
        <button
          onClick={() =>
            ref.current?.setMode(ref.current.getMode() === "2d" ? "3d" : "2d")
          }
        >
          toggle 2d/3d
        </button>
        <button onClick={() => ref.current?.clear()}>clear</button>
      </div>
    </div>
  );
}
export const HeadlessStory: Story = { render: () => <Headless /> };
HeadlessStory.storyName = "Headless (no toolbar)";

// 6. Large graph
function makeLargeGraph(n = 1000): GraphData {
  const nodes = Array.from({ length: n }, (_, i) => ({
    id: i,
    group: i % 8,
  }));
  const links = Array.from({ length: n - 1 }, (_, i) => ({
    source: i + 1,
    target: Math.floor(Math.random() * (i + 1)),
  }));
  return { nodes, links };
}
function LargeGraph() {
  const data = useMemo(() => makeLargeGraph(1000), []);
  return (
    <div style={{ width: "100vw", height: "100vh" }}>
      <LoraGraphCanvas
        defaultData={data}
        nodeAutoColorBy="group"
        cooldownTicks={50}
        warmupTicks={20}
      />
    </div>
  );
}
export const LargeGraphStory: Story = { render: () => <LargeGraph /> };
LargeGraphStory.storyName = "Large (1k nodes)";

// 6b. Stress graph — 10k nodes.
//
// The renderer's `performanceProfile="auto"` (default) picks the
// xlarge tier at this size and injects sensible defaults for
// cooldownTicks / d3AlphaDecay / ngraph-in-3D / lower mesh res —
// see `src/utils/perfTier.ts`. We deliberately leave the full UI
// (toolbar, legend, options menu, mode toggle, selection panel)
// turned on so the stress story doubles as a playground for the
// feature set under load. Toggle 2D ↔ 3D with the mode button or
// the `3` key to feel the perf delta.
function makeStressGraph(n = 10_000): GraphData {
  const nodes = Array.from({ length: n }, (_, i) => ({
    id: i,
    group: i % 12,
  }));
  // Sparse tree so the force solver actually converges. Each node
  // attaches to a random earlier node — keeps the graph connected
  // without producing a hairball.
  const links = Array.from({ length: n - 1 }, (_, i) => ({
    source: i + 1,
    target: Math.floor(Math.random() * (i + 1)),
  }));
  return { nodes, links };
}
function StressGraph() {
  const data = useMemo(() => makeStressGraph(10_000), []);
  return (
    <div style={{ width: "100vw", height: "100vh" }}>
      <LoraGraphCanvas
        defaultData={data}
        nodeAutoColorBy="group"
        nodeRelSize={2}
        showLegend
      />
    </div>
  );
}
export const StressGraphStory: Story = { render: () => <StressGraph /> };
StressGraphStory.storyName = "Stress (10k nodes)";

// 7. DAG layout
function DagStory() {
  const data = useMemo<GraphData>(() => {
    const nodes = Array.from({ length: 20 }, (_, i) => ({ id: i }));
    const links = nodes
      .slice(1)
      .map((n) => ({
        source: n.id,
        target: Math.floor(Math.random() * (Number(n.id) || 1)),
      }));
    return { nodes, links };
  }, []);
  return (
    <div style={{ width: "100vw", height: "100vh" }}>
      <LoraGraphCanvas
        defaultData={data}
        dagMode="td"
        dagLevelDistance={50}
        linkDirectionalArrowLength={4}
        linkDirectionalArrowRelPos={1}
      />
    </div>
  );
}
export const DagStoryStory: Story = { render: () => <DagStory /> };
DagStoryStory.storyName = "DAG layout";

// 8. Theming — dark + custom accent
function CustomTheme() {
  const [accent, setAccent] = useState("#ff6699");
  return (
    <div style={{ width: "100vw", height: "100vh" }}>
      <LoraGraphCanvas
        defaultData={SMALL_GRAPH}
        nodeLabel="id"
        nodeAutoColorBy="group"
        backgroundColor="#0e1014"
        theme={{ ...darkTheme, accent }}
      />
      <div
        style={{
          position: "fixed",
          left: 12,
          bottom: 12,
          display: "flex",
          gap: 6,
          fontFamily: "system-ui, sans-serif",
          fontSize: 12,
          color: "#e6e9ee",
        }}
      >
        Accent:
        <input
          type="color"
          value={accent}
          onChange={(e) => setAccent(e.target.value)}
        />
      </div>
    </div>
  );
}
export const Theming: Story = { render: () => <CustomTheme /> };
Theming.storyName = "Theming (dark + custom accent)";

// 9. Hover-highlight-neighbors
export const HighlightNeighbors: Story = {
  render: () => (
    <div style={{ width: "100vw", height: "100vh" }}>
      <LoraGraphCanvas
        defaultData={SMALL_GRAPH}
        nodeLabel="id"
        nodeAutoColorBy="group"
        highlightNeighborsOnHover
        autoIndexNeighbors
        autoPauseRedraw={false}
      />
    </div>
  ),
};
HighlightNeighbors.storyName = "Hover → highlight neighbors";

// 10. Click-to-focus
export const ClickToFocus: Story = {
  render: () => (
    <div style={{ width: "100vw", height: "100vh" }}>
      <LoraGraphCanvas
        defaultData={SMALL_GRAPH}
        nodeLabel="id"
        nodeAutoColorBy="group"
        focusOnClick
      />
    </div>
  ),
};
ClickToFocus.storyName = "Click a node to focus (click again to restore)";

// 11. Background grid
export const Grid: Story = {
  render: () => (
    <div style={{ width: "100vw", height: "100vh" }}>
      <LoraGraphCanvas
        defaultData={SMALL_GRAPH}
        nodeLabel="id"
        nodeAutoColorBy="group"
        showGrid
        backgroundColor="#fafbfc"
      />
    </div>
  ),
};
Grid.storyName = "Background grid";

// 12. Collision force (no overlap)
export const Collide: Story = {
  render: () => {
    const data = useMemo<GraphData>(() => {
      const nodes = Array.from({ length: 30 }, (_, i) => ({
        id: i,
        group: i % 5,
      }));
      const links = nodes
        .slice(1)
        .map((n) => ({
          source: n.id,
          target: Math.floor(Math.random() * (Number(n.id) || 1)),
        }));
      return { nodes, links };
    }, []);
    return (
      <div style={{ width: "100vw", height: "100vh" }}>
        <LoraGraphCanvas
          defaultData={data}
          nodeRelSize={8}
          nodeAutoColorBy="group"
          collideNodes
        />
      </div>
    );
  },
};
Collide.storyName = "Collision force";

// 13. Group legend
export const Legend: Story = {
  render: () => (
    <div style={{ width: "100vw", height: "100vh" }}>
      <LoraGraphCanvas
        defaultData={SMALL_GRAPH}
        nodeLabel="id"
        nodeAutoColorBy="group"
        showLegend
      />
    </div>
  ),
};
Legend.storyName = "Group legend (click to filter)";

// 14. Beeswarm layout
function BeeswarmStory() {
  const data = useMemo<GraphData>(() => {
    // 300 nodes spread randomly along the x-axis via `pos`.
    const nodes = Array.from({ length: 300 }, (_, i) => ({
      id: i,
      pos: Math.random(),
      group: i % 5,
    }));
    return { nodes, links: [] };
  }, []);
  return (
    <div style={{ width: "100vw", height: "100vh" }}>
      <LoraGraphCanvas
        defaultData={data}
        nodeAutoColorBy="group"
        nodeRelSize={4}
        beeswarm={{
          axis: "x",
          value: (n) => ((n.pos as number) - 0.5) * 800,
        }}
      />
    </div>
  );
}
export const Beeswarm: Story = { render: () => <BeeswarmStory /> };
Beeswarm.storyName = "Beeswarm layout";

// 15. Emit-particle on demand — "ping" along a link every second
function EmitParticleStory() {
  const ref = useRef<LoraGraphCanvasHandle>(null);
  useEffect(() => {
    const id = setInterval(() => {
      const links = ref.current?.getData().links;
      if (!links || links.length === 0) return;
      const link = links[Math.floor(Math.random() * links.length)];
      if (link) ref.current?.emitParticle(link);
    }, 800);
    return () => clearInterval(id);
  }, []);
  return (
    <div style={{ width: "100vw", height: "100vh" }}>
      <LoraGraphCanvas
        ref={ref}
        defaultData={SMALL_GRAPH}
        nodeLabel="id"
        nodeAutoColorBy="group"
        linkDirectionalParticleWidth={3}
      />
    </div>
  );
}
export const EmitParticle: Story = { render: () => <EmitParticleStory /> };
EmitParticle.storyName = "emitParticle() — periodic flow ping";

// 16. Random topology — orphan nodes + at least one hub
function makeRandomGraph(): GraphData {
  const NODE_COUNT = 30;
  const ORPHAN_COUNT = 6;
  const HUB_COUNT = 2;
  const HUB_FANOUT = 8;
  const EXTRA_LINKS = 14;

  const nodes = Array.from({ length: NODE_COUNT }, (_, i) => ({
    id: i,
    group: i % 5,
  }));

  // Shuffle indices, then carve out a disjoint pool of orphans vs
  // linkable nodes. Anyone in the first `ORPHAN_COUNT` slots stays
  // unlinked; the rest are eligible for the hub fan-outs and random
  // edges below.
  const indices = nodes.map((n) => Number(n.id));
  for (let i = indices.length - 1; i > 0; i--) {
    const j = Math.floor(Math.random() * (i + 1));
    [indices[i], indices[j]] = [indices[j]!, indices[i]!];
  }
  const linkable = indices.slice(ORPHAN_COUNT);
  const hubs = linkable.slice(0, HUB_COUNT);

  const links: GraphData["links"] = [];
  const seen = new Set<string>();
  const addLink = (a: number, b: number) => {
    if (a === b) return;
    const key = a < b ? `${a}-${b}` : `${b}-${a}`;
    if (seen.has(key)) return;
    seen.add(key);
    links.push({ source: a, target: b });
  };

  // Each hub gets a fan-out so it visibly anchors a small cluster.
  for (const hub of hubs) {
    for (let k = 0; k < HUB_FANOUT; k++) {
      const other = linkable[Math.floor(Math.random() * linkable.length)]!;
      addLink(hub, other);
    }
  }
  // A handful of extra edges between random non-orphan nodes for noise.
  for (let k = 0; k < EXTRA_LINKS; k++) {
    const a = linkable[Math.floor(Math.random() * linkable.length)]!;
    const b = linkable[Math.floor(Math.random() * linkable.length)]!;
    addLink(a, b);
  }

  return { nodes, links };
}

function RandomGraph() {
  const data = useMemo(() => makeRandomGraph(), []);
  return (
    <div style={{ width: "100vw", height: "100vh" }}>
      <LoraGraphCanvas
        defaultData={data}
        nodeLabel="id"
        nodeAutoColorBy="group"
        backgroundColor="#fafbfc"
      />
    </div>
  );
}
export const RandomGraphStory: Story = { render: () => <RandomGraph /> };
RandomGraphStory.storyName = "Random topology (orphans + hub)";

// 17. Labeled nodes and links — caption comes from the `label` field
// on each datum (no `nodeLabel` / `linkLabel` accessor wired). Links
// without a `label` fall back to the canonical "source → target"
// caption so the demo also shows the precedence in action.
const LABELED_GRAPH: GraphData = {
  nodes: [
    { id: "u1", label: "Alice",   group: "user" },
    { id: "u2", label: "Bob",     group: "user" },
    { id: "u3", label: "Carol",   group: "user" },
    { id: "t1", label: "Roadmap", group: "team" },
    { id: "t2", label: "Infra",   group: "team" },
    { id: "p1", label: "lora-graph-canvas", group: "project" },
    { id: "p2", label: "lora-node",         group: "project" },
  ],
  links: [
    { source: "u1", target: "t1", label: "leads" },
    { source: "u2", target: "t1", label: "member of" },
    { source: "u3", target: "t2", label: "leads" },
    { source: "t1", target: "p1", label: "owns" },
    { source: "t2", target: "p2", label: "owns" },
    // No `label` — falls back to "u1 → p2".
    { source: "u1", target: "p2" },
  ],
};

export const LabeledStory: Story = {
  render: () => (
    <div style={{ width: "100vw", height: "100vh" }}>
      <LoraGraphCanvas
        defaultData={LABELED_GRAPH}
        nodeAutoColorBy="group"
        showLabels
        backgroundColor="#fafbfc"
      />
    </div>
  ),
  parameters: {
    docs: {
      description: {
        story:
          "Nodes and links carry their own `label` field — no `nodeLabel` / `linkLabel` accessor is wired. The renderer prefers `label` over the id-based fallback, so `Alice` renders instead of `u1` and `leads` renders instead of `u1 → t1`. The last link omits `label` and falls back to the canonical `source → target` caption.",
      },
    },
  },
};
LabeledStory.storyName = "Labels on nodes & links";

export const ConfirmDeleteStory: Story = {
  render: () => (
    <div style={{ width: "100vw", height: "100vh" }}>
      <LoraGraphCanvas
        defaultData={{
          nodes: [{ id: "a" }, { id: "b" }, { id: "c" }],
          links: [
            { source: "a", target: "b" },
            { source: "b", target: "c" },
          ],
        }}
        // Synchronous `window.confirm` doubles as a no-dependency demo
        // of the async guard contract — the canvas will await whatever
        // we return, so a sync boolean works the same as a promise.
        onBeforeNodeDelete={(nodes, { source }) =>
          window.confirm(
            `Delete ${nodes.length} node(s) via ${source}?\n${nodes
              .map((n) => `• ${String(n.id)}`)
              .join("\n")}`,
          )
        }
        onBeforeLinkDelete={(links, { source }) =>
          window.confirm(
            `Delete ${links.length} link(s) via ${source}?`,
          )
        }
      />
    </div>
  ),
  parameters: {
    docs: {
      description: {
        story:
          "Wires `onBeforeNodeDelete` / `onBeforeLinkDelete` so every delete path (Backspace, toolbar, context menu, selection panel, cut) routes through a `window.confirm`. Returning `false` cancels — the graph is unchanged. The same guard fires for `handle.removeNode` calls with `source: \"imperative\"`, which a host can short-circuit when it's the caller.",
      },
    },
  },
};
ConfirmDeleteStory.storyName = "Confirm before delete";
