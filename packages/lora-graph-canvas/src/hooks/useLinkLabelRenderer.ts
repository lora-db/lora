import { useEffect, useMemo, useRef } from "react";
import { Object3D, Sprite, type SpriteMaterial } from "three";
import { readAccessor } from "../utils/accessor";
import {
  createTextSprite,
  type SpriteLabelUserData,
} from "../utils/spriteLabel";
import type {
  Accessor,
  GraphMode,
  LinkObject,
  LoraGraphTheme,
  NodeObject,
} from "../types";

export interface UseLinkLabelRendererParams<L extends LinkObject> {
  mode: GraphMode;
  showLabels: boolean;
  selectedLinkSet: ReadonlySet<string | number>;
  /** Links to render labels for on hover (e.g. the directly-hovered
   *  link, or neighbour links of a hovered node). Styled like the
   *  default caption so the selected pill still stands out. */
  hoveredLinkSet?: ReadonlySet<string | number>;
  accentColor: string;
  hostLinkCanvasObject?: (
    l: L,
    ctx: CanvasRenderingContext2D,
    globalScale: number,
  ) => void;
  hostLinkThreeObject?: (l: L) => unknown;
  hostLinkPositionUpdate?:
    | ((
        obj: unknown,
        coords: {
          start: { x: number; y: number; z: number };
          end: { x: number; y: number; z: number };
        },
        link: L,
      ) => void | boolean | null)
    | null;
  linkLabel?: Accessor<string | HTMLElement, L>;
  theme?: Partial<LoraGraphTheme>;
}

export interface LinkLabelRenderer<L> {
  /** 2D: `linkCanvasObject` drawing the label along the link. */
  canvasObject:
    | ((l: L, ctx: CanvasRenderingContext2D, scale: number) => void)
    | undefined;
  /** 3D: `linkThreeObject` returning a sprite for the label. */
  threeObject: ((l: L) => unknown) | undefined;
  /** 3D: `linkPositionUpdate` placing the sprite at the midpoint of
   *  the link each frame. Returns `true` so the kapsule's default
   *  link tube/line update still runs (we're additive, not
   *  replacing). */
  positionUpdate:
    | ((
        obj: unknown,
        coords: {
          start: { x: number; y: number; z: number };
          end: { x: number; y: number; z: number };
        },
        link: L,
      ) => boolean)
    | undefined;
}

/** Draw a label *along* each link (rotated to match the link's
 *  bearing, font auto-sized to fit between the endpoint margins).
 *  Mirrors the canonical force-graph link-label snippet, themed for
 *  selection. Returns `undefined` when nothing demands a label so the
 *  engine prop bag skips the binding entirely on hot paths. 2D only —
 *  the 3D variant lives in `useLinkLabelRenderer3D`. */
export function useLinkLabelRenderer<L extends LinkObject>(
  params: UseLinkLabelRendererParams<L>,
): LinkLabelRenderer<L> {
  const {
    mode,
    showLabels,
    selectedLinkSet,
    hoveredLinkSet,
    accentColor,
    hostLinkCanvasObject,
    hostLinkThreeObject,
    hostLinkPositionUpdate,
    linkLabel,
    theme,
  } = params;

  const tooltipBg = theme?.tooltipBackground ?? "rgba(255, 255, 255, 0.94)";
  const tooltipFg = theme?.tooltipForeground ?? "#1c1f23";

  // Live state ref read inside the (stable) canvasObject below — same
  // rationale as `useLabelRenderer`'s 2D path: identity churn on
  // every hover would propagate through useGraphEngine →
  // applyDiffedProps and force-graph would do a full canvas redraw
  // on each one.
  const linkCanvas2DStateRef = useRef({
    showLabels,
    selectedLinkSet,
    hoveredLinkSet,
    accentColor,
    tooltipBg,
    tooltipFg,
  });
  linkCanvas2DStateRef.current = {
    showLabels,
    selectedLinkSet,
    hoveredLinkSet,
    accentColor,
    tooltipBg,
    tooltipFg,
  };

  const stableLinkCanvasObject = useMemo(() => {
    if (mode !== "2d") return undefined;
    if (hostLinkCanvasObject) return undefined;

    const fontFamily = theme?.fontFamily ?? "system-ui, sans-serif";
    // 4 graph-units — keeps the 2D label visually comparable to the
    // 3D variant (`pixelHeight: 16`). The auto-sizer still caps by
    // available link length, so short links shrink further.
    const MAX_FONT_SIZE = 4;
    // Margin between the endpoint nodes and the start/end of the
    // label, expressed in graph units. Hard-coded because we don't
    // pipe `nodeRelSize` into this hook — the canonical multiplier
    // is `nodeRelSize * 1.5`; we default to a sensible 6.
    const LABEL_NODE_MARGIN = 6;

    return (link: L, ctx: CanvasRenderingContext2D) => {
      const state = linkCanvas2DStateRef.current;
      const isSelected =
        link.id !== undefined && state.selectedLinkSet.has(link.id);
      const isHovered =
        link.id !== undefined &&
        state.hoveredLinkSet?.has(link.id) === true;
      if (!state.showLabels && !isSelected && !isHovered) return;

      // After the simulation ticks, link.source / link.target hold the
      // resolved NodeObject; before that they may still be raw ids.
      const src = link.source as NodeObject | string | number;
      const tgt = link.target as NodeObject | string | number;
      if (typeof src !== "object" || typeof tgt !== "object") return;

      const label = readAccessor<string | HTMLElement, L>(linkLabel, link);
      const explicit =
        typeof label === "string"
          ? label
          : label instanceof HTMLElement
            ? (label.textContent ?? "")
            : "";
      // Fall back to "source → target" using the resolved node ids
      // (matches the canonical force-graph text-links example) so the
      // user sees *something* without having to wire `linkLabel`.
      const text =
        explicit ||
        `${(src as NodeObject).id ?? ""} → ${(tgt as NodeObject).id ?? ""}`;
      if (!text || text === " → ") return;
      const sx = src.x;
      const sy = src.y;
      const tx = tgt.x;
      const ty = tgt.y;
      if (
        sx === undefined ||
        sy === undefined ||
        tx === undefined ||
        ty === undefined
      ) {
        return;
      }

      const dx = tx - sx;
      const dy = ty - sy;
      const linkLen = Math.sqrt(dx * dx + dy * dy);
      const maxTextLen = linkLen - LABEL_NODE_MARGIN * 2;
      if (maxTextLen <= 0) return; // endpoints too close to fit any label

      // Auto-size font: measure at 1px font, then scale up until the
      // text either fits or hits the cap.
      ctx.font = `1px ${fontFamily}`;
      const measured = ctx.measureText(text).width || 1;
      const fontSize = Math.min(MAX_FONT_SIZE, maxTextLen / measured);
      ctx.font = `${fontSize}px ${fontFamily}`;

      // Orient the label along the link; flip back when it would
      // render upside-down so it always reads left-to-right.
      let angle = Math.atan2(dy, dx);
      if (angle > Math.PI / 2) angle = -(Math.PI - angle);
      if (angle < -Math.PI / 2) angle = -(-Math.PI - angle);

      const cx = (sx + tx) / 2;
      const cy = (sy + ty) / 2;
      const textW = ctx.measureText(text).width;
      // More generous padding (horizontal especially) so the pill has
      // breathing room — a tight rect on top of the line is hard to
      // pick out at a glance.
      const padX = fontSize * 0.6;
      const padY = fontSize * 0.35;
      const bgW = textW + padX;
      const bgH = fontSize + padY;
      const radius = Math.min(bgH / 2, fontSize * 0.35);

      ctx.save();
      ctx.translate(cx, cy);
      ctx.rotate(angle);
      // Selection highlight wraps the text in the accent colour
      // background; otherwise use the themed tooltip-like swatch so
      // it reads cleanly on top of the link.
      ctx.fillStyle = isSelected ? state.accentColor : state.tooltipBg;
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
        anyCtx.roundRect(-bgW / 2, -bgH / 2, bgW, bgH, radius);
      } else {
        ctx.rect(-bgW / 2, -bgH / 2, bgW, bgH);
      }
      ctx.fill();
      ctx.textAlign = "center";
      ctx.textBaseline = "middle";
      ctx.fillStyle = isSelected ? "#ffffff" : state.tooltipFg;
      ctx.fillText(text, 0, 0);
      ctx.restore();
    };
  }, [mode, hostLinkCanvasObject, linkLabel, theme?.fontFamily]);

  const hoverSize2D = hoveredLinkSet?.size ?? 0;
  const hasAnyLinkLabel =
    showLabels || selectedLinkSet.size > 0 || hoverSize2D > 0;
  const canvasObject = hasAnyLinkLabel ? stableLinkCanvasObject : undefined;

  // Per-instance registry of live link-label sprites, keyed by link
  // id. Populated by the threeObject accessor; an effect below walks
  // it to flip `sprite.visible` whenever the gating state changes.
  // Same rationale as `useLabelRenderer`: setting `sprite.visible =
  // false` lets three.js skip hidden sprites during scene traversal
  // entirely, where zeroing material.opacity would still pay matrix /
  // transparent-sort / draw-call cost per sprite per frame. The
  // tradeoff is that `onBeforeRender` doesn't fire for invisible
  // sprites, so we have to drive visibility from this effect rather
  // than from inside the render loop.
  const linkSpriteRegistry = useRef<Map<string | number, Sprite>>(
    new Map(),
  );
  // Live ref so a newly-created sprite seeds its visibility from the
  // *current* state on first frame, without dragging selection /
  // hover sets into the accessor memo's deps.
  const linkVisibilityRef = useRef({
    showLabels,
    selectedLinkSet,
    hoveredLinkSet,
  });
  linkVisibilityRef.current = {
    showLabels,
    selectedLinkSet,
    hoveredLinkSet,
  };

  // 3D variant: spawn a billboarded sprite per link. We extend
  // (rather than replace) the kapsule's default link object, so the
  // line / cylinder still renders alongside the label. The position
  // update places the sprite at the link's midpoint each tick.
  const threeObject = useMemo<LinkLabelRenderer<L>["threeObject"]>(() => {
    if (mode !== "3d") return undefined;
    if (hostLinkThreeObject) return undefined;
    const fontFamily = theme?.fontFamily ?? "system-ui, sans-serif";
    const registry = linkSpriteRegistry.current;
    return (link: L) => {
      const src = link.source as NodeObject | string | number;
      const tgt = link.target as NodeObject | string | number;
      const label = readAccessor<string | HTMLElement, L>(linkLabel, link);
      const explicit =
        typeof label === "string"
          ? label
          : label instanceof HTMLElement
            ? (label.textContent ?? "")
            : "";
      // Same source→target fallback as the 2D path so labels show
      // without the host having to wire `linkLabel`.
      const sId =
        typeof src === "object"
          ? (src as NodeObject).id
          : (src as string | number);
      const tId =
        typeof tgt === "object"
          ? (tgt as NodeObject).id
          : (tgt as string | number);
      const text = explicit || `${sId ?? ""} → ${tId ?? ""}`;
      if (!text || text === " → ") return new Object3D();
      // Constant-screen-size via `pixelHeight: 22` — same auto-scaling
      // path as the node label (`pixelHeight: 28`), just a couple of
      // pixels smaller so edge captions sit visually secondary to node
      // names without competing for attention. Both labels grow / shrink
      // in lock-step with the camera, matching the proportional feel of
      // 2D's `fontSize / globalScale` trick.
      //
      // A second "selected" texture is baked alongside the default so
      // the registry effect below can flip `material.map` when the link
      // becomes selected — mirrors the 2D link label (accent fill +
      // white text on selection) and the 3D node label, so the cue is
      // uniform across both modes and both label kinds.
      const sprite = createTextSprite({
        text,
        fontFamily,
        fontSize: 48,
        color: tooltipFg,
        backgroundColor: tooltipBg,
        selectedColor: "#ffffff",
        selectedBackgroundColor: accentColor,
        pixelHeight: 22,
      });
      // Link labels read as secondary annotations: depth-tested so
      // closer geometry (node spheres, nearer link tubes) occludes
      // them — this is the "behind" treatment that distinguishes
      // them from node labels (which sit on top via `depthTest =
      // false` + a very high `renderOrder`). `renderOrder = 10` lifts
      // link labels above the default-0 kapsule geometry so two link
      // labels at similar depth resolve predictably; node labels
      // still win on overlap because they're depth-test-disabled and
      // render much later in the transparent pass.
      (sprite.material as SpriteMaterial).depthTest = true;
      sprite.renderOrder = 10;
      // Seed visibility from current state so a freshly-added link
      // appears (or stays hidden) on its first rendered frame
      // without waiting for the next state change to trigger the
      // registry effect.
      const vState = linkVisibilityRef.current;
      const isSelectedNow =
        link.id !== undefined && vState.selectedLinkSet.has(link.id);
      const isHoveredNow =
        link.id !== undefined &&
        vState.hoveredLinkSet?.has(link.id) === true;
      sprite.visible =
        vState.showLabels || isSelectedNow || isHoveredNow;
      if (link.id !== undefined) {
        registry.set(link.id, sprite);
      }
      return sprite;
    };
  }, [
    mode,
    hostLinkThreeObject,
    linkLabel,
    theme?.fontFamily,
    tooltipBg,
    tooltipFg,
    accentColor,
  ]);

  // Sync sprite.visible + selected texture with the current
  // showLabels / selection / hover state. The texture flip mirrors
  // the 3D node-label registry effect so selection feedback is
  // identical across both label kinds — and the 2D pill cue is
  // visually equivalent on top.
  useEffect(() => {
    if (mode !== "3d") return;
    const registry = linkSpriteRegistry.current;
    for (const [id, sprite] of registry) {
      if (sprite.parent === null) {
        registry.delete(id);
        continue;
      }
      const isSelected = selectedLinkSet.has(id);
      const isHovered = hoveredLinkSet?.has(id) === true;
      sprite.visible = showLabels || isSelected || isHovered;
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
  }, [mode, showLabels, selectedLinkSet, hoveredLinkSet]);

  const positionUpdate = useMemo<LinkLabelRenderer<L>["positionUpdate"]>(
    () => {
      if (mode !== "3d") return undefined;
      if (hostLinkPositionUpdate !== undefined && hostLinkPositionUpdate !== null) {
        // Host owns positioning entirely.
        return undefined;
      }
      return (
        obj: unknown,
        coords: {
          start: { x: number; y: number; z: number };
          end: { x: number; y: number; z: number };
        },
      ): boolean => {
        // Only sprites are repositioned; an empty Object3D (returned
        // when there's no text) has no `.position` to set sensibly,
        // but the kapsule still calls us — guard.
        if (obj instanceof Sprite) {
          obj.position.set(
            (coords.start.x + coords.end.x) / 2,
            (coords.start.y + coords.end.y) / 2,
            (coords.start.z + coords.end.z) / 2,
          );
        }
        // Return true so the kapsule still runs its default
        // link-geometry update — we're additive, not replacing the
        // line/cylinder.
        return true;
      };
    },
    [mode, hostLinkPositionUpdate],
  );

  return { canvasObject, threeObject, positionUpdate };
}
