import { useEffect, useMemo, useRef } from "react";
import { Group, Object3D, Sprite, type SpriteMaterial } from "three";
import { resolveLinkLabelText } from "../utils/accessor";
import {
  createTextSprite,
  disposeLabelSprite,
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
        link.id !== undefined && state.hoveredLinkSet?.has(link.id) === true;
      if (!state.showLabels && !isSelected && !isHovered) return;

      // After the simulation ticks, link.source / link.target hold the
      // resolved NodeObject; before that they may still be raw ids.
      const src = link.source as NodeObject | string | number;
      const tgt = link.target as NodeObject | string | number;
      if (typeof src !== "object" || typeof tgt !== "object") return;

      // resolveLinkLabelText handles the precedence: explicit linkLabel
      // accessor first, then the link's own `label` field, finally a
      // "source → target" caption from the resolved node ids (matches
      // the canonical force-graph text-links example) so the user
      // sees *something* without having to wire `linkLabel`.
      const text = resolveLinkLabelText(linkLabel, link);
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

  // Per-link registry — every link gets a Group up-front (cheap), and
  // the sprite inside is minted on demand. Two sites can mint:
  //
  //   1. The threeObject accessor itself (eager) — covers the case
  //      where the kapsule rebuilds link meshes mid-state-change
  //      (i.e. every digest re-run, which under the current overlay-
  //      state-in-deps wiring happens on every selection/hover tick).
  //      Without eager creation the new Group would render one full
  //      frame with no sprite and labels visibly vanish during
  //      interaction.
  //   2. The visibility effect (backup) — covers state changes that
  //      don't trigger a kapsule digest (e.g. a fresh sprite needed
  //      after the visible-set widened), and handles flipping
  //      `sprite.visible` + selected-texture swap for sprites that
  //      already exist.
  //
  // Either way, sprite creation is paid only for links that actually
  // have to show their caption — at 10k+ links and zero labels visible,
  // the registry still costs only 10k empty Groups (skipped by the
  // renderer's scene traversal), not 10k canvases + 10k GPU textures.
  interface LinkLabelEntry {
    group: Group;
    sprite: Sprite | null;
    text: string;
  }
  const linkLabelRegistry = useRef<Map<string | number, LinkLabelEntry>>(
    new Map(),
  );

  // Live ref so the threeObject accessor can sample current visibility
  // state at digest-time without depending on it in deps (which would
  // cause every hover tick to invalidate the accessor's identity and
  // force the kapsule to recreate every link mesh from scratch — an
  // O(N) tear-down every mouse-move).
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

  // Mount-scoped cleanup so we don't leak CanvasTextures across remounts.
  useEffect(() => {
    const registry = linkLabelRegistry.current;
    return () => {
      for (const entry of registry.values()) {
        if (entry.sprite) disposeLabelSprite(entry.sprite);
      }
      registry.clear();
    };
  }, []);

  // Sprite variant — always-installed under the unified engine (the
  // 2D canvas-object path is dead under propBindings' `only2d` gate).
  // Eagerly mints a sprite for any link that's currently in a visible
  // state at the moment the accessor runs.
  const threeObject = useMemo<LinkLabelRenderer<L>["threeObject"]>(() => {
    if (hostLinkThreeObject) return undefined;
    const registry = linkLabelRegistry.current;
    const fontFamily = theme?.fontFamily ?? "system-ui, sans-serif";
    return (link: L) => {
      const text = resolveLinkLabelText(linkLabel, link);
      const group = new Group();
      const id = link.id;
      if (id !== undefined && text && text !== " → ") {
        const entry: LinkLabelEntry = { group, sprite: null, text };
        registry.set(id, entry);
        // Eager mint when the link is already in a visible state at
        // digest-time. Without this, the new Group spends a frame
        // with no child sprite — visible to the user as the label
        // briefly disappearing whenever the kapsule rebuilds link
        // meshes (which is on every overlay state change now that the
        // wrappers correctly invalidate per-state).
        const v = linkVisibilityRef.current;
        const isSelected = v.selectedLinkSet.has(id);
        const isHovered = v.hoveredLinkSet?.has(id) === true;
        if (v.showLabels || isSelected || isHovered) {
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
          (sprite.material as SpriteMaterial).depthTest = false;
          sprite.renderOrder = 500;
          // Apply the selected texture if appropriate so the first
          // rendered frame doesn't flash the wrong style.
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
    hostLinkThreeObject,
    linkLabel,
    theme?.fontFamily,
    tooltipBg,
    tooltipFg,
    accentColor,
  ]);

  // Visibility / lifecycle effect. Same shape as the node-label
  // version: walk the registry, mint the sprite on first show, swap
  // textures for selection, toggle `visible` otherwise. Sprites once
  // minted are kept around (re-hovering the same link is then free)
  // and only disposed when the kapsule detaches the parent Group.
  useEffect(() => {
    const registry = linkLabelRegistry.current;
    const fontFamily = theme?.fontFamily ?? "system-ui, sans-serif";
    for (const [id, entry] of registry) {
      if (entry.group.parent === null) {
        if (entry.sprite) disposeLabelSprite(entry.sprite);
        registry.delete(id);
        continue;
      }
      const isSelected = selectedLinkSet.has(id);
      const isHovered = hoveredLinkSet?.has(id) === true;
      const shouldShow = showLabels || isSelected || isHovered;
      if (shouldShow && entry.sprite === null) {
        // First-time creation. The expensive canvas rasterise + GPU
        // upload, paid only for links the user is actually inspecting.
        const sprite = createTextSprite({
          text: entry.text,
          fontFamily,
          fontSize: 48,
          color: tooltipFg,
          backgroundColor: tooltipBg,
          selectedColor: "#ffffff",
          selectedBackgroundColor: accentColor,
          pixelHeight: 22,
        });
        // Link labels are always-visible annotations: depth-test off
        // so node spheres / link tubes never occlude the caption, and
        // a high renderOrder so the sprite paints AFTER everything
        // the kapsule injects but still UNDER the node labels
        // (renderOrder 1000). Trade-off: in dense 3D scenes a label
        // can "show through" geometry between camera and link — but
        // a hidden hover label is worse than an over-eager one.
        (sprite.material as SpriteMaterial).depthTest = false;
        sprite.renderOrder = 500;
        entry.group.add(sprite as unknown as Object3D);
        entry.sprite = sprite;
      }
      const sprite = entry.sprite;
      if (sprite) {
        sprite.visible = shouldShow;
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
    selectedLinkSet,
    hoveredLinkSet,
    accentColor,
    tooltipBg,
    tooltipFg,
    theme?.fontFamily,
  ]);

  const positionUpdate = useMemo<LinkLabelRenderer<L>["positionUpdate"]>(() => {
    if (
      hostLinkPositionUpdate !== undefined &&
      hostLinkPositionUpdate !== null
    ) {
      // Host owns positioning entirely.
      return undefined;
    }
    // In 2D presentation the top-down camera makes the world XY
    // plane map 1:1 onto screen XY, so we can drive
    // `material.rotation` (a screen-space CCW radian on the sprite
    // quad) directly from `atan2(dy, dx)` to align the label with
    // the link's bearing. Normalised into [-π/2, π/2] so the text
    // never reads upside-down. In 3D the sprite stays billboarded
    // (rotation = 0) — anchoring along the link in world-space
    // would require unprojecting through the camera each tick and
    // is rarely what the user wants when they can already orbit.
    const is2D = mode === "2d";
    return (
      obj: unknown,
      coords: {
        start: { x: number; y: number; z: number };
        end: { x: number; y: number; z: number };
      },
    ): boolean => {
      // The threeObject is a Group (possibly with a lazily-added
      // sprite child); position the Group at the link midpoint so a
      // freshly-minted sprite picks up the right transform on its
      // first rendered frame. Skip the per-tick work for groups
      // whose sprite hasn't been minted yet AND haven't moved — the
      // empty Group costs the renderer essentially nothing, so we
      // don't need to track its position until there's a sprite to
      // see.
      if (obj instanceof Group) {
        const mx = (coords.start.x + coords.end.x) / 2;
        const my = (coords.start.y + coords.end.y) / 2;
        const mz = (coords.start.z + coords.end.z) / 2;
        obj.position.set(mx, my, mz);
        // One-shot z-order pinning. The kapsule wraps every link in
        // a parent Group and attaches the default line/cylinder mesh
        // alongside our label Group. Default renderOrder is 0
        // everywhere, so in 2D top-down (coplanar geometry) lines
        // z-fight with — and visibly pass through — node spheres.
        // Drop the link's parent to `-1` so nodes (default 0) always
        // draw on top. The userData flag keeps this idempotent
        // across the ~60 ticks/sec this callback fires at.
        const parent = obj.parent;
        if (
          parent &&
          !(parent.userData as { lgcOrderApplied?: boolean }).lgcOrderApplied
        ) {
          (parent.userData as { lgcOrderApplied?: boolean }).lgcOrderApplied =
            true;
          parent.renderOrder = -1;
          for (const sibling of parent.children) {
            if (sibling !== obj) sibling.renderOrder = -1;
          }
        }
        const child = obj.children[0];
        if (child instanceof Sprite) {
          const material = child.material as SpriteMaterial;
          if (is2D) {
            const dx = coords.end.x - coords.start.x;
            const dy = coords.end.y - coords.start.y;
            let angle = Math.atan2(dy, dx);
            if (angle > Math.PI / 2) angle -= Math.PI;
            else if (angle < -Math.PI / 2) angle += Math.PI;
            // Sub-radian no-op guard so we don't dirty the material
            // buffer when the link hasn't visibly moved frame-to-
            // frame.
            if (Math.abs(material.rotation - angle) > 0.001) {
              material.rotation = angle;
            }
          } else if (material.rotation !== 0) {
            // Coming back from 2D — restore the billboarded baseline.
            material.rotation = 0;
          }
        }
      }
      // Return true so the kapsule still runs its default
      // link-geometry update — we're additive, not replacing the
      // line/cylinder.
      return true;
    };
  }, [mode, hostLinkPositionUpdate]);

  return { canvasObject, threeObject, positionUpdate };
}
