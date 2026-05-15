import { describe, it, expect, beforeEach, vi } from "vitest";
import { renderHook, act } from "@testing-library/react";
import { useGraphData } from "../src/hooks/useGraphData";
import { __resetIdCounters } from "../src/utils/ids";
import type { GraphData, NodeObject, LinkObject } from "../src/types";

beforeEach(() => __resetIdCounters());

describe("useGraphData (uncontrolled)", () => {
  it("seeds from defaultData on mount", () => {
    const seed: GraphData = {
      nodes: [{ id: "a" }, { id: "b" }],
      links: [{ source: "a", target: "b" }],
    };
    const { result } = renderHook(() =>
      useGraphData<NodeObject, LinkObject>({ defaultData: seed }),
    );
    expect(result.current.data.nodes).toHaveLength(2);
    expect(result.current.data.links).toHaveLength(1);
  });

  it("addNode appends and returns the created node", () => {
    const { result } = renderHook(() => useGraphData({}));
    let created: NodeObject | undefined;
    act(() => {
      created = result.current.addNode({ label: "hello" });
    });
    expect(created?.id).toBeDefined();
    expect(result.current.data.nodes).toContainEqual(
      expect.objectContaining({ label: "hello" }),
    );
  });

  it("addNode accepts an explicit id and position", () => {
    const { result } = renderHook(() => useGraphData({}));
    act(() => {
      result.current.addNode({ id: 42 }, { at: { x: 10, y: 20 } });
    });
    expect(result.current.data.nodes[0]).toMatchObject({
      id: 42,
      x: 10,
      y: 20,
    });
  });

  it("removeNode also removes attached links (cascade)", () => {
    const seed: GraphData = {
      nodes: [{ id: "a" }, { id: "b" }, { id: "c" }],
      links: [
        { source: "a", target: "b" },
        { source: "b", target: "c" },
      ],
    };
    const { result } = renderHook(() => useGraphData({ defaultData: seed }));
    act(() => result.current.removeNode("b"));
    expect(result.current.data.nodes.map((n) => n.id)).toEqual(["a", "c"]);
    expect(result.current.data.links).toHaveLength(0);
  });

  it("removeNode handles links that point at resolved node objects", () => {
    const a = { id: "a" };
    const b = { id: "b" };
    const seed: GraphData = {
      nodes: [a, b],
      links: [{ source: a, target: b }],
    };
    const { result } = renderHook(() => useGraphData({ defaultData: seed }));
    act(() => result.current.removeNode("a"));
    expect(result.current.data.links).toHaveLength(0);
  });

  it("updateNode patches by id", () => {
    const { result } = renderHook(() =>
      useGraphData({
        defaultData: { nodes: [{ id: "a", label: "x" }], links: [] },
      }),
    );
    act(() => result.current.updateNode("a", { label: "y" }));
    expect(result.current.data.nodes[0]?.label).toBe("y");
  });

  it("removeLink filters by predicate", () => {
    const { result } = renderHook(() =>
      useGraphData({
        defaultData: {
          nodes: [{ id: "a" }, { id: "b" }],
          links: [
            { source: "a", target: "b", id: "l1" },
            { source: "a", target: "b", id: "l2" },
          ],
        },
      }),
    );
    act(() => result.current.removeLink((l) => l.id === "l1"));
    expect(result.current.data.links.map((l) => l.id)).toEqual(["l2"]);
  });

  it("clear empties the graph", () => {
    const { result } = renderHook(() =>
      useGraphData({
        defaultData: {
          nodes: [{ id: "a" }],
          links: [],
        },
      }),
    );
    act(() => result.current.clear());
    expect(result.current.data).toEqual({ nodes: [], links: [] });
  });

  it("fires onChange for every mutation", () => {
    const onChange = vi.fn();
    const { result } = renderHook(() =>
      useGraphData<NodeObject, LinkObject>({
        defaultData: { nodes: [], links: [] },
        onChange,
      }),
    );
    act(() => {
      result.current.addNode({ id: "a" });
    });
    act(() => {
      result.current.addLink({ source: "a", target: "a" });
    });
    expect(onChange).toHaveBeenCalledTimes(2);
    const lastCall = onChange.mock.calls.at(-1)?.[0];
    expect(lastCall.nodes).toHaveLength(1);
    expect(lastCall.links).toHaveLength(1);
  });
});

describe("useGraphData (controlled)", () => {
  it("reflects controlled prop changes between renders", () => {
    const { result, rerender } = renderHook(
      ({ data }: { data: GraphData }) => useGraphData({ controlled: data }),
      {
        initialProps: {
          data: { nodes: [{ id: "a" }], links: [] } as GraphData,
        },
      },
    );
    expect(result.current.data.nodes).toHaveLength(1);
    rerender({
      data: { nodes: [{ id: "a" }, { id: "b" }], links: [] } as GraphData,
    });
    expect(result.current.data.nodes).toHaveLength(2);
  });

  it("still notifies onChange when host mutates via the api", () => {
    const onChange = vi.fn();
    const { result } = renderHook(() =>
      useGraphData<NodeObject, LinkObject>({
        controlled: { nodes: [], links: [] },
        onChange,
      }),
    );
    act(() => result.current.addNode({ id: "a" }));
    expect(onChange).toHaveBeenCalledOnce();
  });
});
