/** @vitest-environment jsdom */
import { describe, it, expect, vi, beforeEach } from "vitest";
import { renderHook } from "@testing-library/react";
import type { CameraState, GraphEngine } from "../src/engines/types";
import type { GraphMode, LinkObject, NodeObject } from "../src/types";

// The persistence behavior used to live in a dedicated hook
// (`useEnginePersistence`). It now lives inside `useGraphEngine`:
// camera state is captured in the mount-effect cleanup before the
// outgoing kapsule is destroyed and restored to each new engine, and
// pause state is applied via a `[engine, paused]` effect. These tests
// exercise that integrated behavior by mocking the engine *adapter*
// factories rather than the underlying kapsules, so we can fully
// control what each engine reports and observe what's applied.

// Shared journal of fake-engine events across factory + test.
const events: string[] = [];

interface FakeEngine extends GraphEngine<NodeObject, LinkObject> {
  _cameraState: CameraState;
  _id: string;
}

function makeFakeEngine(
  id: string,
  mode: GraphMode,
  initialCamera: CameraState,
): FakeEngine {
  let camera = initialCamera;
  const engine: Partial<FakeEngine> = {
    mode,
    get _cameraState() {
      return camera;
    },
    _id: id,
    setGraphData: vi.fn(),
    getGraphData: vi.fn(() => ({ nodes: [], links: [] })),
    fit: vi.fn(),
    centerAt: vi.fn(),
    zoom: vi.fn(),
    getZoom: vi.fn(() => 1),
    screen2Graph: vi.fn((x, y) => ({ x, y, z: 0 })),
    graph2Screen: vi.fn((x, y) => ({ x, y, z: 0 })),
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
    getCameraState: vi.fn(() => {
      events.push(`${id}:getCameraState`);
      return camera;
    }),
    setCameraState: vi.fn((state: CameraState) => {
      events.push(`${id}:setCameraState`);
      camera = state;
    }),
    applyProps: vi.fn(),
    getCanvasElement: vi.fn(() => null),
    destroy: vi.fn(() => {
      events.push(`${id}:destroy`);
    }),
  };
  return engine as FakeEngine;
}

// Queue of pre-built engines the mocked factories will hand out, in
// order. Each test pushes the engines it expects to be mounted.
const engineQueue: FakeEngine[] = [];

vi.mock("../src/engines/createEngine2D", () => ({
  createEngine2D: vi.fn(() => {
    const e = engineQueue.shift();
    if (!e) throw new Error("test bug: 2D engine queue empty");
    return e;
  }),
}));
vi.mock("../src/engines/createEngine3D", () => ({
  createEngine3D: vi.fn(() => {
    const e = engineQueue.shift();
    if (!e) throw new Error("test bug: 3D engine queue empty");
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

describe("useGraphEngine persistence", () => {
  it("restores camera state when a new engine mounts in the same mode", () => {
    const e2d_a = makeFakeEngine("2d-a", "2d", {
      mode: "2d",
      x: 100,
      y: 200,
      k: 3,
    });
    const e3d = makeFakeEngine("3d", "3d", {
      mode: "3d",
      x: 0,
      y: 0,
      z: 999,
      lookAt: { x: 1, y: 2, z: 3 },
    });
    const e2d_b = makeFakeEngine("2d-b", "2d", {
      mode: "2d",
      x: 0,
      y: 0,
      k: 1,
    });
    engineQueue.push(e2d_a, e3d, e2d_b);

    const { rerender } = renderHook(
      (params: { mode: GraphMode }) => useGraphEngine(baseProps(params)),
      { initialProps: { mode: "2d" } },
    );

    // Mount-time has no saved camera yet, so setCameraState shouldn't fire.
    expect(e2d_a.setCameraState).not.toHaveBeenCalled();

    // Swap to 3D — the outgoing 2D engine's cleanup captures its camera.
    rerender({ mode: "3d" });
    expect(e2d_a.getCameraState).toHaveBeenCalled();
    expect(e2d_a.destroy).toHaveBeenCalled();
    // The 3D engine has no saved 3D camera yet.
    expect(e3d.setCameraState).not.toHaveBeenCalled();

    // Swap back to 2D — the new 2D engine should receive the captured snapshot.
    rerender({ mode: "2d" });
    expect(e2d_b.setCameraState).toHaveBeenCalledWith(
      { mode: "2d", x: 100, y: 200, k: 3 },
      0,
    );
  });

  it("keeps 2D and 3D camera snapshots separate", () => {
    const e2d_a = makeFakeEngine("2d-a", "2d", {
      mode: "2d",
      x: 11,
      y: 22,
      k: 5,
    });
    const e3d_a = makeFakeEngine("3d-a", "3d", {
      mode: "3d",
      x: 50,
      y: 60,
      z: 70,
      lookAt: { x: 0, y: 0, z: 0 },
    });
    const e2d_b = makeFakeEngine("2d-b", "2d", {
      mode: "2d",
      x: 0,
      y: 0,
      k: 1,
    });
    const e3d_b = makeFakeEngine("3d-b", "3d", {
      mode: "3d",
      x: 0,
      y: 0,
      z: 0,
      lookAt: { x: 0, y: 0, z: 0 },
    });
    engineQueue.push(e2d_a, e3d_a, e2d_b, e3d_b);

    const { rerender } = renderHook(
      (params: { mode: GraphMode }) => useGraphEngine(baseProps(params)),
      { initialProps: { mode: "2d" } },
    );
    // 2d → 3d → 2d → 3d. Each restoration sees only its own mode's snapshot.
    rerender({ mode: "3d" });
    rerender({ mode: "2d" });
    rerender({ mode: "3d" });

    expect(e2d_b.setCameraState).toHaveBeenCalledWith(
      { mode: "2d", x: 11, y: 22, k: 5 },
      0,
    );
    expect(e3d_b.setCameraState).toHaveBeenCalledWith(
      {
        mode: "3d",
        x: 50,
        y: 60,
        z: 70,
        lookAt: { x: 0, y: 0, z: 0 },
      },
      0,
    );
  });

  it("applies pause on initial mount when paused=true", () => {
    const e = makeFakeEngine("e", "2d", { mode: "2d", x: 0, y: 0, k: 1 });
    engineQueue.push(e);

    renderHook(() => useGraphEngine(baseProps({ mode: "2d", paused: true })));
    expect(e.pause).toHaveBeenCalledTimes(1);
  });

  it("toggles pause/resume without re-mounting the engine", () => {
    const e = makeFakeEngine("e", "2d", { mode: "2d", x: 0, y: 0, k: 1 });
    engineQueue.push(e);

    const { rerender } = renderHook(
      (params: { paused: boolean }) =>
        useGraphEngine(baseProps({ mode: "2d", paused: params.paused })),
      { initialProps: { paused: false } },
    );
    // Initial mount with paused=false fires resume() once.
    expect(e.resume).toHaveBeenCalledTimes(1);
    expect(e.pause).not.toHaveBeenCalled();

    rerender({ paused: true });
    expect(e.pause).toHaveBeenCalledTimes(1);

    rerender({ paused: false });
    expect(e.resume).toHaveBeenCalledTimes(2);
    // Same engine throughout — pause toggles do not trigger destroy.
    expect(e.destroy).not.toHaveBeenCalled();
  });

  it("re-applies paused state to a fresh engine on mode swap", () => {
    const e2d = makeFakeEngine("2d", "2d", { mode: "2d", x: 0, y: 0, k: 1 });
    const e3d = makeFakeEngine("3d", "3d", {
      mode: "3d",
      x: 0,
      y: 0,
      z: 100,
      lookAt: { x: 0, y: 0, z: 0 },
    });
    engineQueue.push(e2d, e3d);

    const { rerender } = renderHook(
      (params: { mode: GraphMode; paused: boolean }) =>
        useGraphEngine(baseProps(params)),
      { initialProps: { mode: "2d", paused: true } },
    );
    expect(e2d.pause).toHaveBeenCalledTimes(1);

    rerender({ mode: "3d", paused: true });
    // The fresh 3D engine inherits the current paused value.
    expect(e3d.pause).toHaveBeenCalledTimes(1);
  });
});
