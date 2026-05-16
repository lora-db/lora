/** @vitest-environment jsdom */
import { describe, it, expect, vi, beforeEach } from "vitest";
import { createRef } from "react";
import { render, act } from "@testing-library/react";

// Capture the event handlers the engines receive so we can drive them
// from the test. The mock engine writes them to module-level slots.
const handlers3D: Record<string, (...a: unknown[]) => void> = {};

function makeFakeKapsule(captureBag: Record<string, (...a: unknown[]) => void>) {
  const captureSetters = new Set([
    "onNodeClick",
    "onNodeRightClick",
    "onBackgroundClick",
    "onBackgroundRightClick",
  ]);
  const noop = vi.fn(function (this: unknown) {
    return this;
  });
  const base = {
    graphData: vi.fn(function (this: unknown, _d?: unknown) {
      return _d ?? { nodes: [], links: [] };
    }),
    _destructor: vi.fn(),
  };
  const handler: ProxyHandler<Record<string, unknown>> = {
    get(target, prop) {
      const key = prop as string;
      if (key === "graphData") return target.graphData;
      if (key === "_destructor") return target._destructor;
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

vi.mock("../../src/engines/3d-force-graph", () => ({
  default: vi.fn(() => makeFakeKapsule(handlers3D)),
}));

beforeEach(() => {
  for (const k of Object.keys(handlers3D)) delete handlers3D[k];
});

const { LoraGraphCanvas } = await import("../../src/LoraGraphCanvas");
import type { LoraGraphCanvasHandle } from "../../src/types";

describe("add-node tool", () => {
  it("creates a node at the projected canvas coords on background click", () => {
    const ref = createRef<LoraGraphCanvasHandle>();
    render(
      <LoraGraphCanvas
        ref={ref}
        width={400}
        height={300}
        defaultData={{ nodes: [], links: [] }}
      />,
    );
    // Activate the add-node tool.
    act(() => {
      ref.current?.setSelection([]); // any noop to flush
    });
    // The tool is set via the React layer's toolbar. We jump straight
    // to the underlying state by sending the keybinding event.
    act(() => {
      window.dispatchEvent(
        new KeyboardEvent("keydown", { key: "n" }),
      );
    });
    // Simulate the engine reporting a background click. (Our mock
    // captured the handler the React layer wired up.)
    act(() => {
      handlers3D.onBackgroundClick?.(
        new MouseEvent("click", { clientX: 50, clientY: 60 }),
      );
    });
    const data = ref.current?.getData();
    expect(data?.nodes).toHaveLength(1);
    expect(data?.nodes[0]).toMatchObject({ x: expect.any(Number) });
  });

  it("background click in select mode clears selection without adding nodes", () => {
    const ref = createRef<LoraGraphCanvasHandle>();
    render(
      <LoraGraphCanvas
        ref={ref}
        width={400}
        height={300}
        defaultData={{ nodes: [{ id: "a" }], links: [] }}
      />,
    );
    act(() => {
      handlers3D.onBackgroundClick?.(
        new MouseEvent("click", { clientX: 50, clientY: 60 }),
      );
    });
    expect(ref.current?.getData().nodes).toHaveLength(1);
  });
});

describe("add-link tool", () => {
  it("creates a link between two clicked nodes", () => {
    const ref = createRef<LoraGraphCanvasHandle>();
    render(
      <LoraGraphCanvas
        ref={ref}
        width={400}
        height={300}
        defaultData={{
          nodes: [{ id: "a" }, { id: "b" }],
          links: [],
        }}
      />,
    );
    // Switch to add-link tool via keybinding.
    act(() => {
      window.dispatchEvent(new KeyboardEvent("keydown", { key: "l" }));
    });
    // Click node A (source), then node B (target).
    act(() => {
      handlers3D.onNodeClick?.(
        { id: "a" },
        new MouseEvent("click"),
      );
    });
    act(() => {
      handlers3D.onNodeClick?.(
        { id: "b" },
        new MouseEvent("click"),
      );
    });
    const links = ref.current?.getData().links;
    expect(links).toHaveLength(1);
    expect(links?.[0]).toMatchObject({ source: "a", target: "b" });
  });

  it("clicking the same node twice cancels the in-progress link", () => {
    const ref = createRef<LoraGraphCanvasHandle>();
    render(
      <LoraGraphCanvas
        ref={ref}
        width={400}
        height={300}
        defaultData={{ nodes: [{ id: "a" }], links: [] }}
      />,
    );
    act(() => {
      window.dispatchEvent(new KeyboardEvent("keydown", { key: "l" }));
    });
    act(() => {
      handlers3D.onNodeClick?.({ id: "a" }, new MouseEvent("click"));
    });
    act(() => {
      handlers3D.onNodeClick?.({ id: "a" }, new MouseEvent("click"));
    });
    expect(ref.current?.getData().links).toHaveLength(0);
  });
});

describe("delete via keybinding", () => {
  it("removes selected nodes and cascades links on Backspace", () => {
    const ref = createRef<LoraGraphCanvasHandle>();
    render(
      <LoraGraphCanvas
        ref={ref}
        width={400}
        height={300}
        defaultData={{
          nodes: [{ id: "a" }, { id: "b" }],
          links: [{ source: "a", target: "b" }],
        }}
      />,
    );
    act(() => {
      ref.current?.setSelection(["a"]);
    });
    act(() => {
      window.dispatchEvent(new KeyboardEvent("keydown", { key: "Backspace" }));
    });
    expect(ref.current?.getData().nodes.map((n) => n.id)).toEqual(["b"]);
    expect(ref.current?.getData().links).toHaveLength(0);
  });
});
