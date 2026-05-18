/** @vitest-environment jsdom */
import { describe, it, expect, vi, beforeEach } from "vitest";
import { renderHook } from "@testing-library/react";
import type { GraphEngine } from "../src/engines/types";
import type {
  GraphMode,
  LinkObject,
  NodeObject,
} from "../src/types";
import type { UnifiedEngine } from "../src/engines/createEngineUnified";

// Under the unified engine, mode flips do not destroy or rebuild the
// engine — they call engine.setMode(target, durationMs) and the
// engine handles the transition internally. These tests mock the
// engine factory and verify that lifecycle: a single engine instance
// persists across mode changes, paused state flows in via React
// props, and setMode is called with the right args when `mode`
// changes.

const events: string[] = [];

interface FakeEngine extends UnifiedEngine<NodeObject, LinkObject> {
  _id: string;
  _setModeCalls: Array<{ target: GraphMode; durationMs?: number }>;
}

function makeFakeEngine(id: string, initialMode: GraphMode): FakeEngine {
  let mode: GraphMode = initialMode;
  const setModeCalls: Array<{ target: GraphMode; durationMs?: number }> = [];
  const fake: Partial<FakeEngine> = {
    get mode() {
      return mode;
    },
    _id: id,
    _setModeCalls: setModeCalls,
    setMode: vi.fn((target: GraphMode, durationMs?: number) => {
      events.push(`${id}:setMode(${target})`);
      setModeCalls.push(
        durationMs === undefined ? { target } : { target, durationMs },
      );
      mode = target;
    }) as unknown as FakeEngine["setMode"],
    setGraphData: vi.fn(),
    getGraphData: vi.fn(() => ({ nodes: [], links: [] })),
    fit: vi.fn(),
    centerAt: vi.fn(),
    zoom: vi.fn(),
    getZoom: vi.fn(() => 1),
    screen2Graph: vi.fn((x: number, y: number) => ({ x, y, z: 0 })),
    graph2Screen: vi.fn((x: number, y: number) => ({ x, y, z: 0 })),
    getGraphBbox: vi.fn(() => ({
      x: [0, 0] as [number, number],
      y: [0, 0] as [number, number],
      z: [0, 0] as [number, number],
    })),
    pause: vi.fn(() => {
      events.push(`${id}:pause`);
    }),
    resume: vi.fn(() => {
      events.push(`${id}:resume`);
    }),
    reheat: vi.fn(),
    resize: vi.fn(),
    d3Force: vi.fn(),
    emitParticle: vi.fn(),
    stopAnimation: vi.fn(),
    focusOn: vi.fn(),
    getCameraState: vi.fn(() => ({ mode: "3d" as const, x: 0, y: 0, z: 100, lookAt: { x: 0, y: 0, z: 0 } })),
    setCameraState: vi.fn(),
    applyProps: vi.fn(),
    getCanvasElement: vi.fn(() => null),
    destroy: vi.fn(() => {
      events.push(`${id}:destroy`);
    }),
  };
  return fake as FakeEngine;
}

// Queue of pre-built engines the mocked factory will hand out. Each
// test pushes the one engine it expects to be mounted (mode flips no
// longer mount fresh engines).
const engineQueue: FakeEngine[] = [];

vi.mock("../src/engines/createEngineUnified", () => ({
  createEngineUnified: vi.fn(() => {
    const e = engineQueue.shift();
    if (!e) throw new Error("test bug: engine queue empty");
    return e;
  }),
}));

const { useGraphEngine } = await import("../src/hooks/useGraphEngine");

const mount = document.createElement("div");

function baseProps(overrides: { mode: GraphMode; paused?: boolean }) {
  return {
    mount,
    width: 100,
    height: 100,
    data: { nodes: [], links: [] },
    props: {} as never,
    paused: overrides.paused ?? false,
    mode: overrides.mode,
  };
}

beforeEach(() => {
  events.length = 0;
  engineQueue.length = 0;
});

describe("useGraphEngine (unified)", () => {
  it("creates the engine once and keeps it across mode flips", () => {
    const engine = makeFakeEngine("e", "2d");
    engineQueue.push(engine);

    const { rerender } = renderHook(
      (params: { mode: GraphMode }) => useGraphEngine(baseProps(params)),
      { initialProps: { mode: "2d" } },
    );

    // First mount: no setMode call yet (it started in the right mode).
    expect(engine._setModeCalls).toHaveLength(0);
    expect(engine.destroy).not.toHaveBeenCalled();

    // Flip to 3D — same engine, setMode invoked.
    rerender({ mode: "3d" });
    expect(engine._setModeCalls).toEqual([
      { target: "3d", durationMs: 800 },
    ]);
    expect(engine.destroy).not.toHaveBeenCalled();

    // Flip back — still the same engine, second setMode call.
    rerender({ mode: "2d" });
    expect(engine._setModeCalls).toEqual([
      { target: "3d", durationMs: 800 },
      { target: "2d", durationMs: 800 },
    ]);
    expect(engine.destroy).not.toHaveBeenCalled();
  });

  it("does not call setMode on first mount even if mode is set", () => {
    const engine = makeFakeEngine("e", "3d");
    engineQueue.push(engine);

    renderHook(() => useGraphEngine(baseProps({ mode: "3d" })));
    expect(engine._setModeCalls).toHaveLength(0);
  });

  it("applies pause on initial mount when paused=true", () => {
    const engine = makeFakeEngine("e", "2d");
    engineQueue.push(engine);

    renderHook(() =>
      useGraphEngine(baseProps({ mode: "2d", paused: true })),
    );
    expect(engine.pause).toHaveBeenCalledTimes(1);
  });

  it("toggles pause/resume without re-mounting the engine", () => {
    const engine = makeFakeEngine("e", "2d");
    engineQueue.push(engine);

    const { rerender } = renderHook(
      (params: { paused: boolean }) =>
        useGraphEngine(baseProps({ mode: "2d", paused: params.paused })),
      { initialProps: { paused: false } },
    );
    expect(engine.resume).toHaveBeenCalledTimes(1);
    expect(engine.pause).not.toHaveBeenCalled();

    rerender({ paused: true });
    expect(engine.pause).toHaveBeenCalledTimes(1);

    rerender({ paused: false });
    expect(engine.resume).toHaveBeenCalledTimes(2);
    expect(engine.destroy).not.toHaveBeenCalled();
  });

  it("destroys the engine on unmount", () => {
    const engine = makeFakeEngine("e", "2d");
    engineQueue.push(engine);

    const { unmount } = renderHook(() =>
      useGraphEngine(baseProps({ mode: "2d" })),
    );
    expect(engine.destroy).not.toHaveBeenCalled();

    unmount();
    expect(engine.destroy).toHaveBeenCalledTimes(1);
  });

  it("does not call setGraphData with the same data reference passed to the factory", () => {
    // Regression for the "nodes never expand on initial load" bug:
    // the factory wrote `initialData` to the kapsule and the React
    // forward-data effect immediately fired `setGraphData()` with the
    // SAME reference. The redundant call would pin every just-seeded
    // node and freeze the simulation before it spread them.
    const engine = makeFakeEngine("e", "2d");
    engineQueue.push(engine);
    const data = { nodes: [{ id: "a" }, { id: "b" }], links: [] };

    renderHook(() =>
      useGraphEngine({
        mount,
        width: 100,
        height: 100,
        data,
        props: {} as never,
        paused: false,
        mode: "2d",
      }),
    );
    expect(engine.setGraphData).not.toHaveBeenCalled();
  });

  it("calls setGraphData when a NEW data reference arrives", () => {
    const engine = makeFakeEngine("e", "2d");
    engineQueue.push(engine);
    const initial = { nodes: [{ id: "a" }], links: [] };

    const { rerender } = renderHook(
      (params: { data: typeof initial }) =>
        useGraphEngine({
          mount,
          width: 100,
          height: 100,
          data: params.data,
          props: {} as never,
          paused: false,
          mode: "2d",
        }),
      { initialProps: { data: initial } },
    );
    expect(engine.setGraphData).not.toHaveBeenCalled();

    const next = { nodes: [{ id: "a" }, { id: "b" }], links: [] };
    rerender({ data: next });
    expect(engine.setGraphData).toHaveBeenCalledTimes(1);
    expect(engine.setGraphData).toHaveBeenCalledWith(next);
  });
});

// Sanity-narrow: the cast above shouldn't drift from the underlying
// type. If GraphEngine widens, this import keeps the link explicit.
void (null as unknown as GraphEngine<NodeObject, LinkObject>);
