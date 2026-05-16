/** @vitest-environment jsdom */
import { describe, it, expect, vi, beforeEach } from "vitest";
import { render } from "@testing-library/react";

// Record every chainable-setter call made against the fake kapsule so
// the assertions can verify which perf knobs reached the engine.
const setterCalls: Array<{ key: string; value: unknown }> = [];

const eventBindingKeys = new Set([
  "onNodeClick",
  "onNodeRightClick",
  "onNodeHover",
  "onNodeDrag",
  "onNodeDragEnd",
  "onLinkClick",
  "onLinkRightClick",
  "onLinkHover",
  "onBackgroundClick",
  "onBackgroundRightClick",
  "onZoom",
  "onZoomEnd",
  "onRenderFramePre",
  "onRenderFramePost",
  "onEngineTick",
  "onEngineStop",
]);

function makeRecordingKapsule() {
  const base = {
    graphData: vi.fn(function (this: unknown, _d?: unknown) {
      return _d ?? { nodes: [], links: [] };
    }),
    _destructor: vi.fn(),
    d3Force: vi.fn(function (this: unknown) {
      return this;
    }),
    emitParticle: vi.fn(),
  };
  const handler: ProxyHandler<Record<string, unknown>> = {
    get(target, prop) {
      const key = prop as string;
      if (key in target) return target[key];
      if (key === "zoom") return vi.fn(() => 1);
      if (key === "cameraPosition")
        return vi.fn(() => ({ x: 0, y: 0, z: 100 }));
      if (key === "screen2GraphCoords")
        return vi.fn((x: number, y: number) => ({ x, y, z: 0 }));
      if (key === "graph2ScreenCoords")
        return vi.fn((x: number, y: number) => ({ x, y, z: 0 }));
      if (key === "getGraphBbox")
        return vi.fn(() => ({ x: [0, 0], y: [0, 0], z: [0, 0] }));
      // Event-binding setters: swallow without recording so the
      // captures don't drown out the perf knobs we actually care about.
      if (eventBindingKeys.has(key)) {
        return function (this: unknown) {
          return this;
        };
      }
      // Every other access is treated as a chainable setter. Record
      // the (key, value) pair on each call.
      return function (this: unknown, value: unknown) {
        setterCalls.push({ key, value });
        return this;
      };
    },
  };
  return new Proxy(base, handler) as unknown as Record<
    string,
    (...args: unknown[]) => unknown
  >;
}

vi.mock("../../src/engines/3d-force-graph", () => ({ default: vi.fn(makeRecordingKapsule) }));

beforeEach(() => {
  setterCalls.length = 0;
});

const { LoraGraphCanvas } = await import("../../src/LoraGraphCanvas");

function nodes(n: number) {
  return Array.from({ length: n }, (_, i) => ({ id: i }));
}

function valuesFor(key: string) {
  return setterCalls.filter((c) => c.key === key).map((c) => c.value);
}

describe("auto performance tuning", () => {
  it("leaves a small 2D graph alone (default tier)", () => {
    render(
      <LoraGraphCanvas
        width={400}
        height={300}
        defaultData={{ nodes: nodes(100), links: [] }}
      />,
    );
    // No tier kicks in below 2k weighted, so cooldownTicks /
    // d3AlphaDecay should not have been pushed to the engine by us.
    expect(valuesFor("cooldownTicks")).not.toContain(100);
    expect(valuesFor("cooldownTicks")).not.toContain(60);
    expect(valuesFor("cooldownTicks")).not.toContain(30);
  });

  it("applies the large-tier defaults at 3k 2D nodes", () => {
    render(
      <LoraGraphCanvas
        width={400}
        height={300}
        defaultData={{ nodes: nodes(3_000), links: [] }}
      />,
    );
    expect(valuesFor("cooldownTicks")).toContain(100);
    expect(valuesFor("d3AlphaDecay")).toContain(0.04);
    // Under the unified Three.js engine, perfTier defaults are no
    // longer mode-specific: 2D-only knobs like `autoPauseRedraw` and
    // `linkLineDash` belonged to the retired Canvas2D path and are no
    // longer emitted. 3D-mode tests below cover the active surface.
  });

  it("applies the huge-tier defaults at 60k 3D nodes", () => {
    render(
      <LoraGraphCanvas
        mode="3d"
        width={400}
        height={300}
        defaultData={{ nodes: nodes(60_000), links: [] }}
      />,
    );
    expect(valuesFor("cooldownTicks")).toContain(30);
    expect(valuesFor("d3AlphaDecay")).toContain(0.15);
    expect(valuesFor("forceEngine")).toContain("ngraph");
    expect(valuesFor("nodeResolution")).toContain(3);
    expect(valuesFor("linkResolution")).toContain(0);
  });

  it("lets host props win over the tier defaults", () => {
    render(
      <LoraGraphCanvas
        width={400}
        height={300}
        defaultData={{ nodes: nodes(3_000), links: [] }}
        cooldownTicks={777}
      />,
    );
    expect(valuesFor("cooldownTicks")).toContain(777);
    expect(valuesFor("cooldownTicks")).not.toContain(100);
  });

  it("respects performanceProfile=\"off\"", () => {
    render(
      <LoraGraphCanvas
        width={400}
        height={300}
        defaultData={{ nodes: nodes(3_000), links: [] }}
        performanceProfile="off"
      />,
    );
    expect(valuesFor("cooldownTicks")).not.toContain(100);
    expect(valuesFor("d3AlphaDecay")).not.toContain(0.04);
  });

  it("respects an explicit profile override (huge on a small graph)", () => {
    render(
      <LoraGraphCanvas
        width={400}
        height={300}
        defaultData={{ nodes: nodes(10), links: [] }}
        performanceProfile="huge"
      />,
    );
    expect(valuesFor("cooldownTicks")).toContain(30);
    expect(valuesFor("d3AlphaDecay")).toContain(0.15);
  });

  it("zeros directional particles + arrows and warmupTicks at non-default tiers", () => {
    render(
      <LoraGraphCanvas
        width={400}
        height={300}
        defaultData={{ nodes: nodes(3_000), links: [] }}
      />,
    );
    expect(valuesFor("linkDirectionalParticles")).toContain(0);
    expect(valuesFor("linkDirectionalArrowLength")).toContain(0);
    expect(valuesFor("warmupTicks")).toContain(0);
  });

  it("throttles the 3D raycaster harder as the tier escalates", () => {
    render(
      <LoraGraphCanvas
        mode="3d"
        width={400}
        height={300}
        defaultData={{ nodes: nodes(60_000), links: [] }}
      />,
    );
    expect(valuesFor("pointerRaycasterThrottleMs")).toContain(200);
  });
});
