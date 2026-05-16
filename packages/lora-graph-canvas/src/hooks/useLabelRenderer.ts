import { useEffect, useMemo, useRef } from "react";
import {
  Group,
  Object3D,
  type Sprite,
  type SpriteMaterial,
} from "three";
import { readAccessor } from "../utils/accessor";
import { createTextSprite, type SpriteLabelUserData } from "../utils/spriteLabel";
import type {
  Accessor,
  GraphMode,
  LoraGraphTheme,
  NodeObject,
} from "../types";

export interface UseLabelRendererParams<N extends NodeObject> {
  mode: GraphMode;
  showLabels: boolean;
  selectedNodeSet: ReadonlySet<string | number>;
  /** Nodes the cursor is currently over (plus optional neighbours). Drawn
   *  with the default caption styling so the hover affordance is visible
   *  without competing with the accent-coloured selection pill. */
  hoveredNodeSet?: ReadonlySet<string | number>;
  accentColor: string;
  hostNodeCanvasObject?: (
    n: N,
    ctx: CanvasRenderingContext2D,
    globalScale: number,
  ) => void;
  hostNodeThreeObject?: (n: N) => unknown;
  nodeLabel?: Accessor<string | HTMLElement, N>;
  nodeVal?: Accessor<number, N>;
  nodeRelSize?: number;
  theme?: Partial<LoraGraphTheme>;
  /** Live nodes list. Required for the 2D post-render pass so labels
   *  can be drawn after all node circles have been painted. Stored in
   *  a ref internally, so identity changes here don't churn the
   *  renderer's outputs. */
  nodes?: ReadonlyArray<N>;
}

export interface NodeLabelRenderer<N> {
  /** 2D mode: kept undefined now — labels draw in `renderFramePost`
   *  so they sit above all node circles, not just the one they're
   *  attached to. The kapsule's `nodeCanvasObject` callback ran
   *  inline with each node's circle paint, so a label drawn early
   *  could be obscured by a circle drawn after. */
  canvasObject:
    | ((n: N, ctx: CanvasRenderingContext2D, scale: number) => void)
    | undefined;
  /** 3D mode: returns an Object3D containing a billboarded text
   *  sprite, positioned below the kapsule's default sphere. Used as
   *  `nodeThreeObject` with `nodeThreeObjectExtend: true` so we
   *  augment rather than replace the default rendering. */
  threeObject: ((n: N) => unknown) | undefined;
  /** 2D mode: walks the node list at the end of every frame and
   *  draws each visible label on top of everything the kapsule has
   *  already painted. Wired into the kapsule's `onRenderFramePost`
   *  callback by `LoraGraphCanvas`. Receives the same world-space
   *  canvas transform as the inline accessor would have, so the
   *  drawing math is identical — just the layering changes. */
  renderFramePost:
    | ((ctx: CanvasRenderingContext2D, globalScale: number) => void)
    | undefined;
}

/** Build the optional `nodeCanvasObject` that draws a small themed pill
 *  with each node's label beneath it. Returns `undefined` when neither
 *  the global `showLabels` flag nor any selection demands labels — that
 *  way the engine prop bag skips the binding entirely on hot paths. */
export function useLabelRenderer<N extends NodeObject>(
  params: UseLabelRendererParams<N>,
): NodeLabelRenderer<N> {
  const {
    mode,
    showLabels,
    selectedNodeSet,
    hoveredNodeSet,
    accentColor,
    hostNodeCanvasObject,
    hostNodeThreeObject,
    nodeLabel,
    nodeVal,
    nodeRelSize,
    theme,
    nodes,
  } = params;

  const tooltipBg = theme?.tooltipBackground ?? "rgba(28, 31, 35, 0.9)";
  const tooltipFg = theme?.tooltipForeground ?? "#ffffff";

  // Live state ref read inside the (stable) canvasObject below so the
  // function identity doesn't change with every hover / selection.
  // Each identity change would propagate through useGraphEngine →
  // applyDiffedProps → the kapsule's nodeCanvasObject setter, which
  // triggers internal state churn + a full-canvas redraw. With this
  // ref, hover-during-mousemove no longer pays that cost.
  const canvas2DStateRef = useRef({
    showLabels,
    selectedNodeSet,
    hoveredNodeSet,
    accentColor,
    tooltipBg,
    tooltipFg,
  });
  canvas2DStateRef.current = {
    showLabels,
    selectedNodeSet,
    hoveredNodeSet,
    accentColor,
    tooltipBg,
    tooltipFg,
  };

  // Stable per-node draw function. Identity depends only on static
  // shape (font, accessors, mode) — never on selection / hover state.
  const stableCanvasObject = useMemo(() => {
    if (mode !== "2d") return undefined;
    if (hostNodeCanvasObject) return undefined;

    const fontFamily = theme?.fontFamily ?? "system-ui, sans-serif";

    return (
      node: N,
      ctx: CanvasRenderingContext2D,
      globalScale: number,
    ) => {
      const state = canvas2DStateRef.current;
      const isSelected =
        node.id !== undefined && state.selectedNodeSet.has(node.id);
      const isHovered =
        node.id !== undefined &&
        state.hoveredNodeSet?.has(node.id) === true;
      if (!state.showLabels && !isSelected && !isHovered) return;

      const label = readAccessor<string | HTMLElement, N>(nodeLabel, node);
      const text =
        typeof label === "string"
          ? label
          : label instanceof HTMLElement
            ? (label.textContent ?? String(node.id))
            : String(node.id);
      if (!text) return;

      // Match force-graph's actual on-canvas radius:
      //   radius = sqrt(nodeVal) * nodeRelSize
      const val = readAccessor<number, N>(nodeVal, node) ?? 1;
      const relSize = nodeRelSize ?? 4;
      const radius = Math.max(0.5, Math.sqrt(Math.max(val, 0)) * relSize);

      const fontSize = 14 / globalScale;
      const padX = 6 / globalScale;
      const padY = 4 / globalScale;

      ctx.font = `${fontSize}px ${fontFamily}`;
      const textW = ctx.measureText(text).width;
      const boxW = textW + padX * 2;
      const boxH = fontSize + padY * 2;
      const cx = node.x ?? 0;
      // Pill sits *above* the node — anchor its bottom edge a full
      // diameter above the node centre so labels clear the edge fan
      // and the node geometry. Force-graph 2D uses screen-style Y
      // (positive = down), so "above" is a negative delta from
      // `node.y`. `cy` is the rect's top, so we subtract boxH again
      // to land the bottom edge in the right place.
      const cy = (node.y ?? 0) - radius * 2 - 4 / globalScale - boxH;

      ctx.fillStyle = isSelected ? state.accentColor : state.tooltipBg;
      const radii = Math.min(boxH / 2, 6 / globalScale);
      const anyCtx = ctx as unknown as {
        roundRect?: (
          x: number,
          y: number,
          w: number,
          h: number,
          r: number,
        ) => void;
      };
      ctx.beginPath();
      if (typeof anyCtx.roundRect === "function") {
        anyCtx.roundRect(cx - boxW / 2, cy, boxW, boxH, radii);
      } else {
        ctx.rect(cx - boxW / 2, cy, boxW, boxH);
      }
      ctx.fill();

      ctx.textAlign = "center";
      ctx.textBaseline = "top";
      ctx.fillStyle = isSelected ? "#ffffff" : state.tooltipFg;
      ctx.fillText(text, cx, cy + padY);
    };
  }, [mode, hostNodeCanvasObject, nodeLabel, nodeVal, nodeRelSize, theme?.fontFamily]);

  // Toggle the prop on/off when there's nothing to draw, so the
  // kapsule skips the per-node call entirely on empty frames. This
  // boundary toggle still fires the kapsule's prop setter (twice per
  // hover lifecycle) but that's vastly cheaper than the per-hover
  // rebuild we did before.
  const hoverSize = hoveredNodeSet?.size ?? 0;
  const hasAnyNodeLabel =
    showLabels || selectedNodeSet.size > 0 || hoverSize > 0;
  const canvasObject = hasAnyNodeLabel ? stableCanvasObject : undefined;

  // Per-instance registry of live sprites keyed by node id. The
  // threeObject accessor populates this when each node sprite is
  // created; an effect below walks it whenever the visibility-driving
  // state changes and flips `sprite.visible` so hidden labels don't
  // get drawn at all.
  //
  // The previous approach gated visibility from inside the sprite's
  // `onBeforeRender` by zeroing `material.opacity`. That kept every
  // hidden sprite in the render list and paid for matrix updates,
  // transparent-sort and a no-op draw call per sprite per frame —
  // which on graphs in the 500-1000-node range was the dominant cost
  // when labels were off. Setting `sprite.visible = false` instead
  // lets three.js skip the object during scene traversal entirely,
  // but means `onBeforeRender` no longer fires for invisible sprites,
  // so we can't re-enable from inside the render loop — hence the
  // outside-the-loop effect.
  const nodeSpriteRegistry = useRef<Map<string | number, Sprite>>(
    new Map(),
  );
  // Live ref to the current visibility-gating state. Read inside the
  // (stable) threeObject accessor so a freshly-created sprite picks
  // up the correct visibility on its first frame, without dragging
  // these sets into the memo deps (which would invalidate the
  // accessor identity on every hover and force the kapsule to
  // destroy + rebuild every sprite).
  const labelVisibilityRef = useRef({
    showLabels,
    selectedNodeSet,
    hoveredNodeSet,
  });
  labelVisibilityRef.current = {
    showLabels,
    selectedNodeSet,
    hoveredNodeSet,
  };

  // 3D variant: produce a billboarded sprite and parent it under the
  // kapsule's default node mesh (we use `nodeThreeObjectExtend: true`
  // upstream so the sphere keeps rendering). The sprite sits above
  // the sphere so it reads as a caption.
  //
  // The accessor identity stays stable across hover/selection changes
  // — those flow through the registry effect below, not through deps
  // — so the kapsule never tears down + rebuilds sprites mid-
  // interaction. (Rebuilds reset the sprite to its default
  // `worldHeight: 4` for one frame before `onBeforeRender` rescales
  // to the pixelHeight target, which is the size-flash users see if
  // we accidentally invalidate the accessor.)
  const threeObject = useMemo<NodeLabelRenderer<N>["threeObject"]>(() => {
    if (mode !== "3d") return undefined;
    if (hostNodeThreeObject) return undefined;

    const fontFamily = theme?.fontFamily ?? "system-ui, sans-serif";
    const registry = nodeSpriteRegistry.current;

    return (node: N) => {
      const label = readAccessor<string | HTMLElement, N>(nodeLabel, node);
      const text =
        typeof label === "string"
          ? label
          : label instanceof HTMLElement
            ? (label.textContent ?? String(node.id))
            : String(node.id);
      if (!text) return new Group();

      // `pixelHeight: 28` keeps the sprite at ~28 CSS px tall regardless
      // of camera distance — mirrors the 2D label's
      // `fontSize / globalScale` trick so panning + zooming feels the
      // same in both modes. Roughly: 14px font + 4px vertical padding
      // × 2 ≈ 22, plus pill rounding → 28.
      //
      // Background colour is locked to the non-selected style: the
      // accent cue lives on the sphere itself via `wrappedNodeColor`,
      // so the label doesn't need to also re-rasterise on every
      // selection change.
      // Selected-state styling bakes a second texture at construction
      // (accent-coloured pill, white text) so the registry effect
      // below can flip `material.map` without re-rasterising on every
      // selection change.
      const sprite = createTextSprite({
        text,
        fontFamily,
        fontSize: 48,
        color: tooltipFg,
        backgroundColor: tooltipBg,
        selectedColor: "#ffffff",
        selectedBackgroundColor: accentColor,
        pixelHeight: 28,
      });
      // Node labels read as the topmost annotation in the scene —
      // never occluded by spheres / links / link labels. `depthTest =
      // false` skips the depth buffer so the sprite pixels always
      // overwrite whatever was there, and a deliberately large
      // `renderOrder` (1000) puts the sprite last in the transparent-
      // sorted pass with enough headroom that nothing the kapsule
      // injects (link tubes, particles, default arrows — all at the
      // default renderOrder of 0) can slip in between us and the link
      // labels at 10.
      (sprite.material as SpriteMaterial).depthTest = false;
      sprite.renderOrder = 1000;
      // Seed visibility from current state so a freshly-added node
      // appears (or stays hidden) on its first rendered frame without
      // waiting for the next state change to trigger the registry
      // effect.
      const vState = labelVisibilityRef.current;
      const isSelectedNow =
        node.id !== undefined && vState.selectedNodeSet.has(node.id);
      const isHoveredNow =
        node.id !== undefined &&
        vState.hoveredNodeSet?.has(node.id) === true;
      sprite.visible =
        vState.showLabels || isSelectedNow || isHoveredNow;
      if (node.id !== undefined) {
        registry.set(node.id, sprite);
      }
      // Position well above the sphere so the caption sits clear of
      // both the node geometry and the edges fanning out of it. The
      // kapsule centres the default node at (0, 0, 0) within the
      // group it builds for our extension, and three.js uses +Y up,
      // so a positive-y offset reads as "above" when the camera is
      // upright. Offset = 2 × radius + 4 graph units: enough
      // headroom for the edge fans on a moderately connected node
      // while still feeling attached to the sphere.
      const val = readAccessor<number, N>(nodeVal, node) ?? 1;
      const relSize = nodeRelSize ?? 4;
      const radius = Math.cbrt(Math.max(val, 0)) * relSize;
      sprite.position.set(0, radius * 2 + 4, 0);
      const group = new Group();
      group.add(sprite as unknown as Object3D);
      return group;
    };
  }, [
    mode,
    hostNodeThreeObject,
    nodeLabel,
    nodeVal,
    nodeRelSize,
    theme?.fontFamily,
    tooltipBg,
    tooltipFg,
    accentColor,
  ]);

  // Sync sprite.visible + selected-style with the current showLabels
  // / selection / hover state. Runs on every state change —
  // O(registry size), which is bounded by node count and only walked
  // here, not per frame.
  //
  // Cleans up stale entries: when the kapsule destroys a sprite
  // (data update, accessor reset), it detaches it from the scene and
  // `sprite.parent` becomes null. We drop those rather than calling
  // .visible on a stranded reference.
  useEffect(() => {
    if (mode !== "3d") return;
    const registry = nodeSpriteRegistry.current;
    for (const [id, sprite] of registry) {
      if (sprite.parent === null) {
        registry.delete(id);
        continue;
      }
      const isSelected = selectedNodeSet.has(id);
      const isHovered = hoveredNodeSet?.has(id) === true;
      sprite.visible = showLabels || isSelected || isHovered;
      // Swap the rasterised texture so selected labels render with
      // the accent-coloured pill + white text. Both textures were
      // baked at sprite construction; this is just a uniform rebind
      // — no canvas work, no GC.
      const data = sprite.userData as SpriteLabelUserData;
      const desired =
        isSelected && data.selectedTexture
          ? data.selectedTexture
          : data.normalTexture;
      const material = sprite.material as SpriteMaterial;
      if (desired && material.map !== desired) {
        material.map = desired;
        material.needsUpdate = true;
      }
    }
  }, [mode, showLabels, selectedNodeSet, hoveredNodeSet]);

  return { canvasObject, threeObject };
}
