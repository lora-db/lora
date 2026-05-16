/** @vitest-environment jsdom */
import { describe, it, expect, vi, beforeEach } from "vitest";
import { createRef } from "react";
import { render, act } from "@testing-library/react";

// Capture d3Force / emitParticle calls and the engine handlers so we
// can verify that the React layer hooks them up correctly.
const handlers2D: Record<string, (...a: unknown[]) => void> = {};
const d3ForceCalls: Array<{ name: string; fn: unknown }> = [];
const emitParticleCalls: unknown[] = [];

function makeFakeKapsule(
  captureBag: Record<string, (...a: unknown[]) => void>,
  d3Bag: typeof d3ForceCalls,
  emitBag: typeof emitParticleCalls,
) {
  const captureSetters = new Set([
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
  ]);
  const noop = vi.fn(function (this: unknown) {
    return this;
  });
  const base = {
    graphData: vi.fn(function (this: unknown, _d?: unknown) {
      return _d ?? { nodes: [], links: [] };
    }),
    _destructor: vi.fn(),
    d3Force: vi.fn(function (this: unknown, name: string, fn: unknown) {
      if (fn !== undefined) d3Bag.push({ name, fn });
      return this;
    }),
    emitParticle: vi.fn((link: unknown) => emitBag.push(link)),
  };
  const handler: ProxyHandler<Record<string, unknown>> = {
    get(target, prop) {
      const key = prop as string;
      if (key === "graphData") return target.graphData;
      if (key === "_destructor") return target._destructor;
      if (key === "d3Force") return target.d3Force;
      if (key === "emitParticle") return target.emitParticle;
      if (key === "zoom") return vi.fn(() => 1);
      if (key === "cameraPosition")
        return vi.fn(() => ({ x: 0, y: 0, z: 100 }));
      if (key === "screen2GraphCoords")
        return vi.fn((x: number, y: number) => ({ x, y, z: 0 }));
      if (key === "graph2ScreenCoords")
        return vi.fn((x: number, y: number) => ({ x, y, z: 0 }));
      if (key === "getGraphBbox")
        return vi.fn(() => ({ x: [0, 0], y: [0, 0], z: [0, 0] }));
      if (captureSetters.has(key)) {
        return function (this: unknown, fn: (...a: unknown[]) => void) {
          captureBag[key] = fn;
          return this;
        };
      }
      return noop;
    },
  };
  return new Proxy(base, handler) as unknown as Record<
    string,
    (...args: unknown[]) => unknown
  >;
}

vi.mock("force-graph", () => ({
  default: vi.fn(() =>
    makeFakeKapsule(handlers2D, d3ForceCalls, emitParticleCalls),
  ),
}));
vi.mock("3d-force-graph", () => ({
  default: vi.fn(() => makeFakeKapsule({}, [], [])),
}));

beforeEach(() => {
  for (const k of Object.keys(handlers2D)) delete handlers2D[k];
  d3ForceCalls.length = 0;
  emitParticleCalls.length = 0;
});

const { LoraGraphCanvas } = await import("../../src/LoraGraphCanvas");
import type { LoraGraphCanvasHandle } from "../../src/types";

describe("d3Force / emitParticle ref methods", () => {
  it("forwards d3Force(name, fn) to the engine", () => {
    const ref = createRef<LoraGraphCanvasHandle>();
    render(
      <LoraGraphCanvas
        ref={ref}
        width={400}
        height={300}
        defaultData={{ nodes: [{ id: "a" }], links: [] }}
      />,
    );
    const fakeForce = vi.fn();
    act(() => {
      ref.current?.d3Force("custom", fakeForce);
    });
    expect(d3ForceCalls).toContainEqual({ name: "custom", fn: fakeForce });
  });

  it("forwards emitParticle(link) to the engine", () => {
    const ref = createRef<LoraGraphCanvasHandle>();
    render(
      <LoraGraphCanvas
        ref={ref}
        width={400}
        height={300}
        defaultData={{
          nodes: [{ id: "a" }, { id: "b" }],
          links: [{ source: "a", target: "b", id: "l1" }],
        }}
      />,
    );
    const link = { source: "a", target: "b", id: "l1" };
    act(() => {
      ref.current?.emitParticle(link);
    });
    expect(emitParticleCalls).toContainEqual(link);
  });
});

describe("collideNodes prop", () => {
  it("registers a collide force when enabled", () => {
    render(
      <LoraGraphCanvas
        width={400}
        height={300}
        defaultData={{ nodes: [{ id: "a" }], links: [] }}
        collideNodes
      />,
    );
    expect(d3ForceCalls.some((c) => c.name === "collide" && c.fn)).toBe(
      true,
    );
  });

  it("removes the force when disabled later", () => {
    const { rerender } = render(
      <LoraGraphCanvas
        width={400}
        height={300}
        defaultData={{ nodes: [{ id: "a" }], links: [] }}
        collideNodes
      />,
    );
    d3ForceCalls.length = 0;
    rerender(
      <LoraGraphCanvas
        width={400}
        height={300}
        defaultData={{ nodes: [{ id: "a" }], links: [] }}
        collideNodes={false}
      />,
    );
    expect(d3ForceCalls.some((c) => c.name === "collide" && c.fn === null))
      .toBe(true);
  });
});

describe("autoIndexNeighbors", () => {
  it("populates _neighbors and _links on each node", () => {
    const data = {
      nodes: [{ id: "a" }, { id: "b" }, { id: "c" }],
      links: [
        { source: "a", target: "b", id: "ab" },
        { source: "b", target: "c", id: "bc" },
      ],
    };
    render(
      <LoraGraphCanvas
        width={400}
        height={300}
        defaultData={data}
        autoIndexNeighbors
      />,
    );
    const b = data.nodes[1] as unknown as {
      _neighbors: Array<{ id: string }>;
      _links: Array<{ id: string }>;
    };
    // b is connected to a and c.
    expect(b._neighbors.map((n) => n.id).sort()).toEqual(["a", "c"]);
    expect(b._links.map((l) => l.id).sort()).toEqual(["ab", "bc"]);
  });
});

describe("click-vs-drag tolerance", () => {
  // jsdom doesn't ship PointerEvent — synthesize one as a MouseEvent
  // re-typed to the pointer event name. The shim only reads `type` and
  // `clientX/Y`, so this is faithful enough.
  const pointer = (type: string, clientX: number, clientY: number) =>
    new MouseEvent(type, { bubbles: true, clientX, clientY });

  // The shim stops pointermove propagation while the cursor is still
  // within the dead-zone, so the kapsule's drag-detection (registered
  // on a descendant of the mount) never fires for tiny mouse jitter.
  it("swallows pointermove events within the dead-zone after the press-grace window", () => {
    // Mock the clock so we can step past the press-grace window
    // deterministically — synthetic events all fire in the same
    // microtask otherwise, which keeps us inside the grace where every
    // move is suppressed regardless of distance.
    const nowSpy = vi.spyOn(performance, "now").mockReturnValue(0);
    try {
      const { container } = render(
        <LoraGraphCanvas
          width={400}
          height={300}
          defaultData={{ nodes: [{ id: "a" }], links: [] }}
        />,
      );
      const mount = container.querySelector(
        ".lgc-engine-mount",
      ) as HTMLElement;
      expect(mount).toBeTruthy();

      // Simulate the kapsule's container-level listener. With the shim
      // active, this listener should not see pointermove events that
      // happen within the dead-zone.
      const kapsuleListener = vi.fn();
      mount.addEventListener("pointermove", kapsuleListener);

      nowSpy.mockReturnValue(0);
      mount.dispatchEvent(pointer("pointerdown", 100, 100));

      // Step past the press-grace window so distance-based filtering
      // governs the rest of the gesture.
      nowSpy.mockReturnValue(200);

      // Tiny jitter — inside the dead-zone.
      mount.dispatchEvent(pointer("pointermove", 102, 101));
      expect(kapsuleListener).not.toHaveBeenCalled();

      // Real movement — beyond the dead-zone.
      mount.dispatchEvent(pointer("pointermove", 130, 130));
      expect(kapsuleListener).toHaveBeenCalledTimes(1);

      // After the dead-zone is exceeded, subsequent moves flow through
      // even if they happen to land back inside the original radius.
      mount.dispatchEvent(pointer("pointermove", 101, 101));
      expect(kapsuleListener).toHaveBeenCalledTimes(2);
    } finally {
      nowSpy.mockRestore();
    }
  });

  // The press-grace window is unconditional — for the first 60 ms after
  // pointerdown the shim swallows *every* move, regardless of distance.
  // That way the involuntary jitter spike that fires the moment a
  // button physically engages can never flip the kapsule's drag flag
  // and convert a click into a drag.
  it("swallows moves during the press-grace window regardless of distance", () => {
    const nowSpy = vi.spyOn(performance, "now").mockReturnValue(0);
    try {
      const { container } = render(
        <LoraGraphCanvas
          width={400}
          height={300}
          defaultData={{ nodes: [{ id: "a" }], links: [] }}
        />,
      );
      const mount = container.querySelector(
        ".lgc-engine-mount",
      ) as HTMLElement;
      const kapsuleListener = vi.fn();
      mount.addEventListener("pointermove", kapsuleListener);

      nowSpy.mockReturnValue(0);
      mount.dispatchEvent(pointer("pointerdown", 100, 100));

      // Still inside the grace window — large move should still be
      // suppressed.
      nowSpy.mockReturnValue(20);
      mount.dispatchEvent(pointer("pointermove", 300, 300));
      expect(kapsuleListener).not.toHaveBeenCalled();

      // Past the grace window — the same magnitude of motion now
      // commits to a drag and propagates.
      nowSpy.mockReturnValue(200);
      mount.dispatchEvent(pointer("pointermove", 320, 320));
      expect(kapsuleListener).toHaveBeenCalledTimes(1);
    } finally {
      nowSpy.mockRestore();
    }
  });

  it("resets the dead-zone on pointerup so the next press gets its own tolerance", () => {
    const { container } = render(
      <LoraGraphCanvas
        width={400}
        height={300}
        defaultData={{ nodes: [{ id: "a" }], links: [] }}
      />,
    );
    const mount = container.querySelector(".lgc-engine-mount") as HTMLElement;
    const kapsuleListener = vi.fn();
    mount.addEventListener("pointermove", kapsuleListener);

    // First gesture: press, jitter, release.
    mount.dispatchEvent(pointer("pointerdown", 50, 50));
    mount.dispatchEvent(pointer("pointermove", 51, 50));
    mount.dispatchEvent(pointer("pointerup", 51, 50));
    expect(kapsuleListener).not.toHaveBeenCalled();

    // Hover-style pointermove with no button pressed should always
    // pass through (no active gesture).
    mount.dispatchEvent(pointer("pointermove", 80, 80));
    expect(kapsuleListener).toHaveBeenCalledTimes(1);

    // Second gesture starts fresh — small movement is again
    // suppressed.
    mount.dispatchEvent(pointer("pointerdown", 200, 200));
    mount.dispatchEvent(pointer("pointermove", 201, 201));
    expect(kapsuleListener).toHaveBeenCalledTimes(1);
  });
});
