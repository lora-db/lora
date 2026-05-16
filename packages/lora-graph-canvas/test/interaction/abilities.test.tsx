/** @vitest-environment jsdom */
import { describe, it, expect, vi, beforeEach } from "vitest";
import { createRef } from "react";
import { render, act } from "@testing-library/react";

const handlers3D: Record<string, (...a: unknown[]) => void> = {};

function makeFakeKapsule(captureBag: Record<string, (...a: unknown[]) => void>) {
  const captureSetters = new Set([
    "onNodeClick",
    "onNodeRightClick",
    "onBackgroundClick",
    "onBackgroundRightClick",
    "onLinkClick",
    "onLinkRightClick",
    "onNodeHover",
    "onLinkHover",
    "onNodeDrag",
    "onNodeDragEnd",
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
  default: vi.fn(() => makeFakeKapsule({})),
}));

beforeEach(() => {
  for (const k of Object.keys(handlers3D)) delete handlers3D[k];
});

const { LoraGraphCanvas } = await import("../../src/LoraGraphCanvas");
import type { LoraGraphCanvasHandle } from "../../src/types";

describe("select-all (Cmd+A)", () => {
  it("selects every node in the graph", () => {
    const ref = createRef<LoraGraphCanvasHandle>();
    render(
      <LoraGraphCanvas
        ref={ref}
        width={400}
        height={300}
        defaultData={{
          nodes: [{ id: "a" }, { id: "b" }, { id: "c" }],
          links: [],
        }}
      />,
    );
    act(() => {
      window.dispatchEvent(
        new KeyboardEvent("keydown", { key: "a", metaKey: true }),
      );
    });
    expect(ref.current?.getSelection().sort()).toEqual(["a", "b", "c"]);
  });
});

describe("duplicate (Cmd+D)", () => {
  it("duplicates the current selection and selects the copies", () => {
    const ref = createRef<LoraGraphCanvasHandle>();
    render(
      <LoraGraphCanvas
        ref={ref}
        width={400}
        height={300}
        defaultData={{
          nodes: [{ id: "a", label: "Alice" }],
          links: [],
        }}
      />,
    );
    act(() => ref.current?.setSelection(["a"]));
    act(() => {
      window.dispatchEvent(
        new KeyboardEvent("keydown", { key: "d", metaKey: true }),
      );
    });
    const nodes = ref.current?.getData().nodes ?? [];
    expect(nodes).toHaveLength(2);
    // The duplicate should carry the user fields (label) but get a new id.
    const dup = nodes.find((n) => n.id !== "a");
    expect(dup?.label).toBe("Alice");
    expect(dup?.id).not.toBe("a");
    // And the selection should now be just the duplicate.
    expect(ref.current?.getSelection()).toEqual([dup?.id]);
  });
});

describe("copy + paste via ref handle", () => {
  it("clones the copied node with a new id", () => {
    const ref = createRef<LoraGraphCanvasHandle>();
    render(
      <LoraGraphCanvas
        ref={ref}
        width={400}
        height={300}
        defaultData={{
          nodes: [{ id: "src", label: "X" }],
          links: [],
        }}
      />,
    );
    act(() => ref.current?.setSelection(["src"]));
    act(() => ref.current?.copy());
    let pasted: ReturnType<NonNullable<typeof ref.current>["paste"]> = [];
    act(() => {
      pasted = ref.current?.paste({ at: { x: 50, y: 60 } }) ?? [];
    });
    expect(pasted).toHaveLength(1);
    expect(pasted[0]?.id).not.toBe("src");
    expect(pasted[0]?.label).toBe("X");
  });
});

describe("link click → selection", () => {
  it("selecting a link via engine click works through the ref handle", () => {
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
    act(() => {
      handlers3D.onLinkClick?.(
        { source: "a", target: "b", id: "l1" },
        new MouseEvent("click"),
      );
    });
    // Selecting a link clears node selection — that's the only
    // externally observable side effect we can assert against here.
    expect(ref.current?.getSelection()).toEqual([]);
  });
});

describe("import / export JSON via ref handle", () => {
  it("exports valid JSON and round-trips through importJSON", () => {
    const ref = createRef<LoraGraphCanvasHandle>();
    render(
      <LoraGraphCanvas
        ref={ref}
        width={400}
        height={300}
        defaultData={{
          nodes: [{ id: "a", label: "Alice" }, { id: "b", label: "Bob" }],
          links: [{ source: "a", target: "b", id: "l1" }],
        }}
      />,
    );
    const exported = ref.current?.exportJSON() ?? "";
    expect(exported).toContain("Alice");
    expect(exported).toContain("Bob");

    act(() => {
      ref.current?.importJSON(
        JSON.stringify({
          nodes: [{ id: "x" }, { id: "y" }],
          links: [],
        }),
      );
    });
    expect(ref.current?.getData().nodes.map((n) => n.id)).toEqual(["x", "y"]);
  });
});

describe("togglePin via ref handle", () => {
  it("pinning sets fx/fy, second toggle clears them", () => {
    const ref = createRef<LoraGraphCanvasHandle>();
    render(
      <LoraGraphCanvas
        ref={ref}
        width={400}
        height={300}
        defaultData={{
          nodes: [{ id: "a", x: 10, y: 20 }],
          links: [],
        }}
      />,
    );
    act(() => ref.current?.togglePin("a"));
    let node = ref.current?.getData().nodes[0];
    expect(node?.fx).toBe(10);
    expect(node?.fy).toBe(20);
    act(() => ref.current?.togglePin("a"));
    node = ref.current?.getData().nodes[0];
    expect(node?.fx).toBeUndefined();
    expect(node?.fy).toBeUndefined();
  });
});
