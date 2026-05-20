import { useEffect, useMemo, useRef } from "react";
import { Group, Object3D, type Sprite, type SpriteMaterial } from "three";
import { readAccessor, resolveNodeLabelText } from "../utils/accessor";
import {
  createTextSprite,
  disposeLabelSprite,
  type SpriteLabelUserData,
} from "../utils/spriteLabel";
import type { Accessor, GraphMode, LoraGraphTheme, NodeObject } from "../types";

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
  } = params;
  // `params.nodes` is reserved on the param type for the upcoming 2D
  // post-render pass (see the field's docstring) but not consumed here
  // yet. Don't destructure it — the unused-vars rule has no
  // `varsIgnorePattern` configured, so even `_nodes` would trip it.

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

    return (node: N, ctx: CanvasRenderingContext2D, globalScale: number) => {
      const state = canvas2DStateRef.current;
      const isSelected =
        node.id !== undefined && state.selectedNodeSet.has(node.id);
      const isHovered =
        node.id !== undefined && state.hoveredNodeSet?.has(node.id) === true;
      if (!state.showLabels && !isSelected && !isHovered) return;

      const text = resolveNodeLabelText(nodeLabel, node);
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
  }, [
    mode,
    hostNodeCanvasObject,
    nodeLabel,
    nodeVal,
    nodeRelSize,
    theme?.fontFamily,
  ]);

  // Toggle the prop on/off when there's nothing to draw, so the
  // kapsule skips the per-node call entirely on empty frames. This
  // boundary toggle still fires the kapsule's prop setter (twice per
  // hover lifecycle) but that's vastly cheaper than the per-hover
  // rebuild we did before.
  const hoverSize = hoveredNodeSet?.size ?? 0;
  const hasAnyNodeLabel =
    showLabels || selectedNodeSet.size > 0 || hoverSize > 0;
  const canvasObject = hasAnyNodeLabel ? stableCanvasObject : undefined;

  // Per-node registry — Group-per-node up-front, sprite minted on
  // demand. Two sites can mint:
  //
  //   1. The threeObject accessor itself (eager) — covers the case
  //      where the kapsule rebuilds node meshes mid-state-change.
  //      Without eager creation the new Group renders one full frame
  //      with no sprite, so node labels visibly flicker / disappear
  //      during hover/select interactions.
  //   2. The visibility effect (backup) — handles state changes that
  //      don't trigger a kapsule digest, plus visibility flips and
  //      selected-texture swaps for sprites that already exist.
  //
  // The deferred-creation pattern is essential at 10k+ nodes: the old
  // unconditional eager mint did `createTextSprite` × 10k on mount,
  // freezing the main thread on canvas rasterise + GPU texture upload
  // for several seconds. The empty-Group fallback keeps cold-mount
  // cheap while still letting hover/select interactions paint sprites
  // on the same frame as the state change.
  interface NodeLabelEntry {
    group: Group;
    sprite: Sprite | null;
    text: string;
    /** Y-offset in world units — pre-computed at registration time so
     *  the (rare) sprite-creation path doesn't re-walk node.val. */
    yOffset: number;
  }
  const nodeLabelRegistry = useRef<Map<string | number, NodeLabelEntry>>(
    new Map(),
  );

  // Live ref so the threeObject accessor can sample current visibility
  // state without depending on it in deps. Same rationale as the link
  // label renderer: putting selection/hover sets in deps would force
  // the kapsule to recreate every node mesh from scratch on every
  // mouse tick, an O(N) tear-down we can avoid by reading through a
  // ref instead.
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

  // Per-mount cleanup: dispose every entry's sprite resources so we
  // don't leak GPU memory across remounts.
  useEffect(() => {
    const registry = nodeLabelRegistry.current;
    return () => {
      for (const entry of registry.values()) {
        if (entry.sprite) disposeLabelSprite(entry.sprite);
      }
      registry.clear();
    };
  }, []);

  // Sprite variant: returns an empty Group up-front for every node and
  // eagerly mints a sprite if the node is currently in a visible state.
  // The Group is parented under the kapsule's default node mesh (via
  // `nodeThreeObjectExtend: true`) so the sphere keeps rendering.
  //
  // Active in BOTH presentation modes — the unified engine renders
  // through the 3D kapsule even in 2D, so the only label path the
  // kapsule actually invokes is `nodeThreeObject`. The 2D
  // `nodeCanvasObject` binding (`canvasObject` below) is `only2d` in
  // propBindings and never reaches the engine; sprites are the only
  // working hover-affordance in 2D.
  const threeObject = useMemo<NodeLabelRenderer<N>["threeObject"]>(() => {
    if (hostNodeThreeObject) return undefined;
    const registry = nodeLabelRegistry.current;
    const fontFamily = theme?.fontFamily ?? "system-ui, sans-serif";
    return (node: N) => {
      const id = node.id;
      const text = resolveNodeLabelText(nodeLabel, node);
      const val = readAccessor<number, N>(nodeVal, node) ?? 1;
      const relSize = nodeRelSize ?? 4;
      // Cube root mirrors three-forcegraph's sphere radius formula —
      // the offset keeps the caption clear of the geometry regardless
      // of how large the user scales individual nodes.
      const radius = Math.cbrt(Math.max(val, 0)) * relSize;
      const yOffset = radius * 2 + 4;
      const group = new Group();
      if (id !== undefined && text) {
        const entry: NodeLabelEntry = {
          group,
          sprite: null,
          text,
          yOffset,
        };
        registry.set(id, entry);
        // Eager mint when the node is already in a visible state at
        // digest-time — see the linkLabelRenderer comment for why
        // waiting for the visibility effect leaves the new Group with
        // no sprite for a frame.
        const v = labelVisibilityRef.current;
        const isSelected = v.selectedNodeSet.has(id);
        const isHovered = v.hoveredNodeSet?.has(id) === true;
        if (v.showLabels || isSelected || isHovered) {
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
          (sprite.material as SpriteMaterial).depthTest = false;
          sprite.renderOrder = 1000;
          sprite.position.set(0, yOffset, 0);
          if (isSelected) {
            const data = sprite.userData as SpriteLabelUserData;
            if (data.selectedTexture) {
              const mat = sprite.material as SpriteMaterial;
              mat.map = data.selectedTexture;
              mat.needsUpdate = true;
            }
          }
          group.add(sprite as unknown as Object3D);
          entry.sprite = sprite;
        }
      }
      return group;
    };
  }, [
    hostNodeThreeObject,
    nodeLabel,
    nodeVal,
    nodeRelSize,
    theme?.fontFamily,
    tooltipBg,
    tooltipFg,
    accentColor,
  ]);

  // Visibility / lifecycle effect. Runs once per state change and
  // walks the registry (O(node count) in the worst case, but the hot
  // case — hover-only — fans out to one entry per state flip). For
  // each entry:
  //
  //   - if the kapsule destroyed our Group (`group.parent === null`)
  //     we drop the entry and dispose its sprite to free GPU memory;
  //   - if the node should currently show a label and no sprite has
  //     been minted yet, we rasterise one and add it to the Group;
  //   - if the sprite already exists, we just flip `.visible` and
  //     (when selected) swap to the pre-baked accent texture.
  //
  // Sprites once minted are kept around even after the label hides,
  // so re-hovering the same node never pays the canvas-rasterise cost
  // again. The teardown path on Group destruction is the only place
  // we actually call `dispose()`.
  useEffect(() => {
    const registry = nodeLabelRegistry.current;
    const fontFamily = theme?.fontFamily ?? "system-ui, sans-serif";
    for (const [id, entry] of registry) {
      if (entry.group.parent === null) {
        if (entry.sprite) disposeLabelSprite(entry.sprite);
        registry.delete(id);
        continue;
      }
      const isSelected = selectedNodeSet.has(id);
      const isHovered = hoveredNodeSet?.has(id) === true;
      const shouldShow = showLabels || isSelected || isHovered;
      if (shouldShow && entry.sprite === null) {
        // Lazy first-time creation. The expensive part — measureText +
        // a fresh CanvasTexture, paid only for nodes that actually
        // need to be readable right now.
        const sprite = createTextSprite({
          text: entry.text,
          fontFamily,
          fontSize: 48,
          color: tooltipFg,
          backgroundColor: tooltipBg,
          selectedColor: "#ffffff",
          selectedBackgroundColor: accentColor,
          pixelHeight: 28,
        });
        // Node labels read as the topmost annotation: depth-test
        // disabled + a very large renderOrder so neither link tubes
        // (renderOrder 0) nor link labels (renderOrder 10) can slip
        // in front.
        (sprite.material as SpriteMaterial).depthTest = false;
        sprite.renderOrder = 1000;
        sprite.position.set(0, entry.yOffset, 0);
        entry.group.add(sprite as unknown as Object3D);
        entry.sprite = sprite;
      }
      const sprite = entry.sprite;
      if (sprite) {
        sprite.visible = shouldShow;
        // Swap the rasterised texture so selected labels render with
        // the accent-coloured pill + white text. Both textures were
        // baked at sprite construction — this is a uniform rebind, no
        // canvas work, no GC.
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
    }
  }, [
    showLabels,
    selectedNodeSet,
    hoveredNodeSet,
    accentColor,
    tooltipBg,
    tooltipFg,
    theme?.fontFamily,
  ]);

  return { canvasObject, threeObject, renderFramePost: undefined };
}
