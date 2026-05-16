import { describe, it, expect, vi } from "vitest";
import { renderHook } from "@testing-library/react";
import { useAccessorOverrides } from "../src/hooks/useAccessorOverrides";
import type { NodeObject } from "../src/types";

interface TestNode extends NodeObject {
  val?: number;
  x?: number;
  y?: number;
}

const empty = new Set<string | number>();

function baseParams() {
  return {
    mode: "2d" as const,
    accentColor: "#4f8ef7",
    selectedNodeSet: empty as ReadonlySet<string | number>,
    selectedLinkSet: empty as ReadonlySet<string | number>,
    highlightNeighborsOnHover: false,
    highlightedNodeIds: empty as ReadonlySet<string | number>,
    highlightedLinkIds: empty as ReadonlySet<string | number>,
    hoverNodeId: null as string | number | null,
    hiddenGroups: new Set<string>() as ReadonlySet<string>,
  };
}

/** Mock just enough of CanvasRenderingContext2D to capture the radius
 *  the wrapper passes to `arc`. */
function makeFakeCtx() {
  const arcCalls: Array<{ x: number; y: number; r: number }> = [];
  const ctx = {
    fillStyle: "",
    beginPath: vi.fn(),
    fill: vi.fn(),
    arc: vi.fn((x: number, y: number, r: number) =>
      arcCalls.push({ x, y, r }),
    ),
  } as unknown as CanvasRenderingContext2D;
  return { ctx, arcCalls };
}

describe("useAccessorOverrides.nodePointerAreaPaint", () => {
  it("installs a 2D shadow-paint wrapper even when nothing is selected", () => {
    const { result } = renderHook(() =>
      useAccessorOverrides<TestNode, never>({ ...baseParams() }),
    );
    // A stable wrapper at all times means the kapsule's throttled
    // shadow-canvas refresh isn't re-triggered on every selection
    // click — that was the cause of stale hit-tests on huge graphs.
    expect(result.current.nodePointerAreaPaint).toBeTypeOf("function");
  });

  it("paints the shadow at the *original* val even when wrappedNodeVal enlarges the visible node", () => {
    const selected = new Set<string | number>(["a"]);
    const { result } = renderHook(() =>
      useAccessorOverrides<TestNode, never>({
        ...baseParams(),
        nodeVal: 1,
        nodeRelSize: 4,
        selectedNodeSet: selected as ReadonlySet<string | number>,
      }),
    );
    const paint = result.current.nodePointerAreaPaint!;
    const wrappedVal = result.current.nodeVal!;
    // Visible val is enlarged for selection (2.25× by current tier),
    // but the shadow paint uses the host's base val (1).
    const visibleVal = (
      typeof wrappedVal === "function"
        ? (wrappedVal as (n: TestNode) => number)({ id: "a", val: 1 })
        : 1
    );
    expect(visibleVal).toBeGreaterThan(1);

    const { ctx, arcCalls } = makeFakeCtx();
    paint({ id: "a", val: 1, x: 0, y: 0 }, "#010203", ctx, 1);
    // Expected radius: sqrt(1) * 4 + 1/scale = 5 px.
    expect(arcCalls).toHaveLength(1);
    expect(arcCalls[0]!.r).toBeCloseTo(5, 5);
  });

  it("includes the +1/globalScale anti-aliasing padding so the colorTracker can match boundary pixels", () => {
    const { result } = renderHook(() =>
      useAccessorOverrides<TestNode, never>({
        ...baseParams(),
        nodeVal: 1,
        nodeRelSize: 2,
      }),
    );
    const paint = result.current.nodePointerAreaPaint!;
    const { ctx, arcCalls } = makeFakeCtx();
    // At globalScale=2, the screen-space 1-pixel pad shrinks to 0.5
    // graph units.
    paint({ id: "a", val: 1, x: 10, y: 20 }, "#000", ctx, 2);
    // Expected radius: sqrt(1) * 2 + 1/2 = 2.5 px.
    expect(arcCalls[0]!.r).toBeCloseTo(2.5, 5);
    expect(arcCalls[0]!.x).toBe(10);
    expect(arcCalls[0]!.y).toBe(20);
  });

  it("defers to a host-supplied nodePointerAreaPaint instead of wrapping", () => {
    const hostPaint = vi.fn();
    const { result } = renderHook(() =>
      useAccessorOverrides<TestNode, never>({
        ...baseParams(),
        nodePointerAreaPaint: hostPaint,
      }),
    );
    expect(result.current.nodePointerAreaPaint).toBe(hostPaint);
  });

  it("returns the host's accessor (or undefined) for 3D — raycaster reads the actual mesh", () => {
    const { result } = renderHook(() =>
      useAccessorOverrides<TestNode, never>({
        ...baseParams(),
        mode: "3d",
      }),
    );
    expect(result.current.nodePointerAreaPaint).toBeUndefined();
  });

  it("uses a modest selected-node val multiplier so the visible node grows without dominating the canvas", () => {
    const selected = new Set<string | number>(["a"]);
    const { result } = renderHook(() =>
      useAccessorOverrides<TestNode, never>({
        ...baseParams(),
        nodeVal: 1,
        selectedNodeSet: selected as ReadonlySet<string | number>,
      }),
    );
    const wrappedVal = result.current.nodeVal as (n: TestNode) => number;
    // Tier-2 sizing: ~2.25× val → roughly 1.5× radius in 2D. Anything
    // far above this re-creates the swallow-clicks bug on dense
    // graphs since `nodeVal` also feeds the kapsule's hit-test path
    // (mitigated in 2D by `nodePointerAreaPaint`, but in 3D the
    // raycaster has no decoupling).
    expect(wrappedVal({ id: "a", val: 1 })).toBeCloseTo(2.25, 5);
    expect(wrappedVal({ id: "b", val: 1 })).toBe(1);
  });
});
