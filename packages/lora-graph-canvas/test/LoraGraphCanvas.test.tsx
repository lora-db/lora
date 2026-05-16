/** @vitest-environment jsdom */
import { describe, it, expect, vi, beforeEach } from "vitest";
import { createRef } from "react";
import { render, act } from "@testing-library/react";

// The unified engine instantiates a real Three.js renderer on
// construction, which crashes jsdom's canvas. Mock 3d-force-graph
// (the only renderer the unified engine touches) to a thin spy so we
// can mount the React layer without a rendering surface.
const fakeInstance = makeFakeKapsule();

vi.mock("../src/engines/3d-force-graph", () => {
  return {
    default: vi.fn(() => fakeInstance),
  };
});

function makeFakeKapsule() {
  const noop = vi.fn(function (this: unknown) {
    return this;
  });
  const value = vi.fn(function (this: unknown) {
    return 1;
  });
  // Any method the adapter calls returns `this` (chainable). The
  // adapter never inspects return values except for the getter-side
  // overload of `zoom()` / `cameraPosition()`, which we stub explicitly.
  const handler: ProxyHandler<Record<string, unknown>> = {
    get(target, prop) {
      if (prop === "graphData") return target.graphData;
      if (prop === "_destructor") return target._destructor;
      if (prop === "zoom") return value;
      if (prop === "cameraPosition")
        return vi.fn(() => ({ x: 0, y: 0, z: 100 }));
      if (prop === "getGraphBbox")
        return vi.fn(() => ({
          x: [0, 0],
          y: [0, 0],
          z: [0, 0],
        }));
      if (prop === "screen2GraphCoords")
        return vi.fn((x: number, y: number) => ({ x, y, z: 0 }));
      if (prop === "graph2ScreenCoords")
        return vi.fn((x: number, y: number) => ({ x, y, z: 0 }));
      return noop;
    },
  };
  const base = {
    graphData: vi.fn(function (this: unknown, _d?: unknown) {
      return _d ?? { nodes: [], links: [] };
    }),
    _destructor: vi.fn(),
  };
  return new Proxy(base, handler) as unknown as Record<
    string,
    (...args: unknown[]) => unknown
  >;
}

beforeEach(() => {
  vi.clearAllMocks();
});

// Import after mocks are set up so the engines pull in the stubs.
const { LoraGraphCanvas } = await import("../src/LoraGraphCanvas");
import type { LoraGraphCanvasHandle } from "../src/types";

describe("<LoraGraphCanvas /> ref handle", () => {
  it("mounts in 2d mode by default and exposes getMode", () => {
    const ref = createRef<LoraGraphCanvasHandle>();
    render(<LoraGraphCanvas ref={ref} width={400} height={300} />);
    expect(ref.current?.getMode()).toBe("2d");
  });

  it("addNode goes through the data api", () => {
    const ref = createRef<LoraGraphCanvasHandle>();
    const onDataChange = vi.fn();
    render(
      <LoraGraphCanvas
        ref={ref}
        width={400}
        height={300}
        defaultData={{ nodes: [], links: [] }}
        onDataChange={onDataChange}
      />,
    );
    act(() => {
      ref.current?.addNode({ id: "x" });
    });
    expect(ref.current?.getData().nodes).toHaveLength(1);
    expect(onDataChange).toHaveBeenCalled();
  });

  it("setMode switches engines and preserves data", () => {
    const ref = createRef<LoraGraphCanvasHandle>();
    render(
      <LoraGraphCanvas
        ref={ref}
        width={400}
        height={300}
        defaultData={{ nodes: [{ id: "a" }], links: [] }}
      />,
    );
    expect(ref.current?.getMode()).toBe("2d");
    act(() => ref.current?.setMode("3d"));
    expect(ref.current?.getMode()).toBe("3d");
    expect(ref.current?.getData().nodes).toHaveLength(1);
  });

  it("removeNode cascades and propagates through the ref handle", () => {
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
    act(() => ref.current?.removeNode("a"));
    expect(ref.current?.getData().nodes.map((n) => n.id)).toEqual(["b"]);
    expect(ref.current?.getData().links).toHaveLength(0);
  });

  it("clear empties the graph", () => {
    const ref = createRef<LoraGraphCanvasHandle>();
    render(
      <LoraGraphCanvas
        ref={ref}
        width={400}
        height={300}
        defaultData={{
          nodes: [{ id: "a" }],
          links: [],
        }}
      />,
    );
    act(() => ref.current?.clear());
    expect(ref.current?.getData()).toEqual({ nodes: [], links: [] });
  });
});
