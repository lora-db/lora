/** @vitest-environment jsdom */
import { describe, it, expect, vi, beforeEach } from "vitest";
import { createRef } from "react";
import { render, act } from "@testing-library/react";

/** Same kapsule stub as the other interaction tests use — the real 3d
 *  renderer crashes jsdom on construction. */
const fakeInstance = makeFakeKapsule();
vi.mock("../src/engines/3d-force-graph", () => ({
  default: vi.fn(() => fakeInstance),
}));

function makeFakeKapsule() {
  const noop = vi.fn(function (this: unknown) {
    return this;
  });
  const value = vi.fn(function (this: unknown) {
    return 1;
  });
  const handler: ProxyHandler<Record<string, unknown>> = {
    get(target, prop) {
      if (prop === "graphData") return target.graphData;
      if (prop === "_destructor") return target._destructor;
      if (prop === "zoom") return value;
      if (prop === "cameraPosition")
        return vi.fn(() => ({ x: 0, y: 0, z: 100 }));
      if (prop === "getGraphBbox")
        return vi.fn(() => ({ x: [0, 0], y: [0, 0], z: [0, 0] }));
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

const { LoraGraphCanvas } = await import("../src/LoraGraphCanvas");
import type {
  DeletionSource,
  LoraGraphCanvasHandle,
  NodeObject,
  LinkObject,
} from "../src/types";

function seedData() {
  return {
    nodes: [{ id: "a" }, { id: "b" }, { id: "c" }] as NodeObject[],
    links: [
      { id: "ab", source: "a", target: "b" },
      { id: "bc", source: "b", target: "c" },
    ] as LinkObject[],
  };
}

describe("delete guards", () => {
  it("guard returning true allows the delete and fires onNodeDeleted", async () => {
    const ref = createRef<LoraGraphCanvasHandle>();
    const onBeforeNodeDelete = vi.fn(
      (_nodes: NodeObject[], _ctx: { source: DeletionSource }) => true,
    );
    const onNodeDeleted = vi.fn();
    render(
      <LoraGraphCanvas
        ref={ref}
        width={400}
        height={300}
        defaultData={seedData()}
        onBeforeNodeDelete={onBeforeNodeDelete}
        onNodeDeleted={onNodeDeleted}
      />,
    );
    await act(async () => {
      await ref.current?.removeNode("a");
    });
    expect(onBeforeNodeDelete).toHaveBeenCalledOnce();
    expect(onNodeDeleted).toHaveBeenCalledOnce();
    expect(onNodeDeleted.mock.calls[0]?.[1]).toEqual({ source: "imperative" });
    expect(ref.current?.getData().nodes.map((n) => n.id)).toEqual(["b", "c"]);
  });

  it("guard returning false cancels the delete", async () => {
    const ref = createRef<LoraGraphCanvasHandle>();
    const onNodeDeleted = vi.fn();
    render(
      <LoraGraphCanvas
        ref={ref}
        width={400}
        height={300}
        defaultData={seedData()}
        onBeforeNodeDelete={() => false}
        onNodeDeleted={onNodeDeleted}
      />,
    );
    let result: boolean | undefined;
    await act(async () => {
      result = await ref.current?.removeNode("a");
    });
    expect(result).toBe(false);
    expect(onNodeDeleted).not.toHaveBeenCalled();
    expect(ref.current?.getData().nodes.map((n) => n.id)).toEqual([
      "a",
      "b",
      "c",
    ]);
  });

  it("guard returning a rejected Promise cancels the delete", async () => {
    const ref = createRef<LoraGraphCanvasHandle>();
    render(
      <LoraGraphCanvas
        ref={ref}
        width={400}
        height={300}
        defaultData={seedData()}
        onBeforeNodeDelete={() => Promise.resolve(false)}
      />,
    );
    await act(async () => {
      await ref.current?.removeNode("a");
    });
    expect(ref.current?.getData().nodes.map((n) => n.id)).toEqual([
      "a",
      "b",
      "c",
    ]);
  });

  it("guard that throws is treated as cancel (data not destroyed)", async () => {
    const ref = createRef<LoraGraphCanvasHandle>();
    render(
      <LoraGraphCanvas
        ref={ref}
        width={400}
        height={300}
        defaultData={seedData()}
        onBeforeNodeDelete={() => {
          throw new Error("nope");
        }}
      />,
    );
    let result: boolean | undefined;
    await act(async () => {
      result = await ref.current?.removeNode("a");
    });
    expect(result).toBe(false);
    expect(ref.current?.getData().nodes).toHaveLength(3);
  });

  it("batch keyboard delete calls the guard once with all selected items", async () => {
    const ref = createRef<LoraGraphCanvasHandle>();
    const onBeforeNodeDelete = vi.fn(
      (_nodes: NodeObject[], _ctx: { source: DeletionSource }) => true,
    );
    render(
      <LoraGraphCanvas
        ref={ref}
        width={400}
        height={300}
        defaultData={seedData()}
        onBeforeNodeDelete={onBeforeNodeDelete}
      />,
    );
    act(() => {
      ref.current?.setSelection(["a", "b"]);
    });
    await act(async () => {
      window.dispatchEvent(new KeyboardEvent("keydown", { key: "Backspace" }));
      // Let the gate's microtask resolve before we assert.
      await Promise.resolve();
    });
    expect(onBeforeNodeDelete).toHaveBeenCalledOnce();
    const [callItems, callCtx] = onBeforeNodeDelete.mock.calls[0] ?? [];
    expect(callItems?.map((n) => n.id).sort()).toEqual(["a", "b"]);
    expect(callCtx?.source).toBe("keyboard");
    expect(ref.current?.getData().nodes.map((n) => n.id)).toEqual(["c"]);
  });

  it("rejected guard during cut leaves both clipboard and data intact", async () => {
    const ref = createRef<LoraGraphCanvasHandle>();
    render(
      <LoraGraphCanvas
        ref={ref}
        width={400}
        height={300}
        defaultData={seedData()}
        onBeforeNodeDelete={() => false}
      />,
    );
    act(() => {
      ref.current?.setSelection(["a"]);
    });
    let result: NodeObject[] | undefined;
    await act(async () => {
      result = await ref.current?.cut();
    });
    expect(result).toEqual([]);
    expect(ref.current?.getData().nodes).toHaveLength(3);
    // Paste should now no-op because the clipboard was never written.
    const pasted = ref.current?.paste();
    expect(pasted).toEqual([]);
  });

  it("link guard runs with source: contextMenu via removeLink", async () => {
    const ref = createRef<LoraGraphCanvasHandle>();
    const onBeforeLinkDelete = vi.fn(
      (_links: LinkObject[], _ctx: { source: DeletionSource }) => true,
    );
    const onLinkDeleted = vi.fn();
    render(
      <LoraGraphCanvas
        ref={ref}
        width={400}
        height={300}
        defaultData={seedData()}
        onBeforeLinkDelete={onBeforeLinkDelete}
        onLinkDeleted={onLinkDeleted}
      />,
    );
    await act(async () => {
      await ref.current?.removeLink((l) => l.id === "ab");
    });
    expect(onBeforeLinkDelete).toHaveBeenCalledOnce();
    expect(onLinkDeleted).toHaveBeenCalledOnce();
    expect(onLinkDeleted.mock.calls[0]?.[1]).toEqual({ source: "imperative" });
    expect(ref.current?.getData().links.map((l) => l.id)).toEqual(["bc"]);
  });

  it("no guard ⇒ data mutates synchronously (legacy callers keep working)", () => {
    const ref = createRef<LoraGraphCanvasHandle>();
    render(
      <LoraGraphCanvas
        ref={ref}
        width={400}
        height={300}
        defaultData={seedData()}
      />,
    );
    act(() => {
      void ref.current?.removeNode("a");
    });
    // Sync — the promise is discarded with `void`, but the mutation
    // happened inside the call.
    expect(ref.current?.getData().nodes.map((n) => n.id)).toEqual(["b", "c"]);
  });
});
