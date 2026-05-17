// Unified 2D ↔ 3D engine: one Three.js renderer + one d3-force-3d
// simulation, always running in 3D space. "2D mode" is a
// presentation overlay — camera is locked top-down, every node has
// its z degree of freedom pinned to 0, orbit controls are disabled.
// Mode switches don't tear down the engine; they tween the camera +
// release/apply the z-pin so nodes physically lift out of (or settle
// back into) the xy-plane while the user watches.
//
// Why this shape:
//   - Three.js can't easily share a canvas with Canvas2D, so a "real"
//     in-place transition needs both modes to live in the same WebGL
//     context. Picking 3D for both keeps the existing 3D engine
//     fully functional and treats 2D as a constrained view of it.
//   - `three-render-objects` (the camera layer under 3d-force-graph)
//     only supports trackball / orbit / fly controls and a
//     perspective camera. We approximate orthographic 2D by parking
//     the perspective camera at a long focal distance — far enough
//     that foreshortening across the visible scene is negligible for
//     graph-viz purposes. If the foreshortening becomes a problem
//     later, the upgrade path is to vendor three-render-objects and
//     add an `OrthographicCamera` mode.
//
// LORA: replaced the previous split createEngine2D.ts (Canvas2D) and
// createEngine3D.ts (3D-only) engines. This unified engine is the only
// runtime engine path now.

import { MOUSE } from "three";
import ForceGraph3DKapsule from "./3d-force-graph";
import { runAnim, lerp, easeInOutCubic } from "./rafAnim";
import {
  EVENT_BINDINGS,
  applyDiffedProps,
  type EventName,
} from "./propBindings";
import type {
  CameraState,
  CreateEngineOptions,
  GraphEngine,
} from "./types";
import type {
  GraphData,
  GraphMode,
  LinkObject,
  LoraGraphCanvasProps,
  NodeObject,
} from "../types";

type Kapsule3D = Record<string, (...args: unknown[]) => unknown> & {
  graphData: (data?: unknown) => unknown;
  _destructor: () => void;
};

interface Coords3D {
  x: number;
  y: number;
  z: number;
  lookAt?: { x: number; y: number; z: number };
}

/** Camera distance for 2D top-down view. Far enough that perspective
 *  foreshortening is below ~1% across a typical viewport — i.e.
 *  visually flat — while still leaving headroom for `zoom`. */
const TWO_D_CAMERA_DISTANCE = 1200;

/** Default camera position for fresh 3D mode (or first mount). */
const DEFAULT_3D_DISTANCE = 300;

export interface UnifiedEngine<
  N extends NodeObject = NodeObject,
  L extends LinkObject = LinkObject,
> extends GraphEngine<N, L> {
  /** Switch presentation mode in place. Animates the camera + the
   *  per-node z constraints; does not destroy the engine. */
  setMode(mode: GraphMode, durationMs?: number): void;
}

export function createEngineUnified<
  N extends NodeObject,
  L extends LinkObject,
>(
  mount: HTMLElement,
  opts: CreateEngineOptions<N, L> & { initialMode: GraphMode },
  handlerRef: { current: LoraGraphCanvasProps<N, L> },
): UnifiedEngine<N, L> {
  // Use orbit controls — they give us clean per-axis toggles
  // (enableRotate/enablePan/enableZoom) and let us remap mouse
  // buttons. Trackball is the upstream default; for a unified
  // engine that has to feel "2D" sometimes, orbit is a better fit.
  const instance = new (ForceGraph3DKapsule as unknown as new (
    el: HTMLElement,
    opts?: { controlType: "trackball" | "orbit" | "fly" },
  ) => Kapsule3D)(mount, { controlType: "orbit" }) as Kapsule3D;

  instance.width!(opts.width);
  instance.height!(opts.height);

  // Wire event handlers through the trampoline so latest React props
  // always win without re-binding on every render.
  for (const name of EVENT_BINDINGS) {
    const setter = instance[name as keyof Kapsule3D];
    if (typeof setter !== "function") continue;
    setter.call(instance, (...args: unknown[]) => {
      const fn = handlerRef.current[name as EventName] as
        | ((...a: unknown[]) => void)
        | undefined;
      if (typeof fn === "function") fn(...args);
    });
  }

  // Initial prop pass — diff against an empty bag so every supported
  // prop fires once.
  applyDiffedProps(
    instance as unknown as Record<string, (value: unknown) => unknown>,
    opts.initialProps as unknown as LoraGraphCanvasProps<NodeObject, LinkObject>,
    {} as LoraGraphCanvasProps<NodeObject, LinkObject>,
    "3d",
  );

  instance.graphData(opts.initialData);

  let currentMode: GraphMode = opts.initialMode;
  let cachedDistance =
    currentMode === "2d" ? TWO_D_CAMERA_DISTANCE : DEFAULT_3D_DISTANCE;
  // Last 3D camera position the user actually inhabited (i.e. before
  // a transition to 2D). Restored when switching back so a user's
  // orbit isn't lost across a 2D detour.
  let last3DCamera: Coords3D | null = null;

  // Handle to the in-flight RAF animation, if any (focus / camera
  // tween / mode transition). Held so a new interaction can preempt
  // a running animation cleanly.
  let cancelAnim: (() => void) | null = null;

  // ── Mode application ──────────────────────────────────────────
  // Apply the static parts of a mode (orbit controls, fz pins).
  // Camera positioning is handled separately so it can be tweened.

  /** Apply control behaviour for the given mode. Pan/zoom semantics
   *  differ between 2D and 3D:
   *
   *  2D (top-down):
   *    - Left-drag = pan (familiar from any 2D viewer)
   *    - Wheel    = zoom toward the cursor (Figma/Miro-style)
   *    - Pan in screen space (so dragging right always moves the
   *      scene right, regardless of camera tilt — which is zero
   *      here, but the flag also disables the world-XZ pan plane
   *      that orbit would otherwise use)
   *    - No rotation
   *    - Slightly snappier zoom speed; 2D viewers feel sluggish
   *      with the orbit default of 1.0
   *
   *  3D (orbit):
   *    - Left-drag = rotate around the lookAt target
   *    - Right-drag = pan
   *    - Wheel    = dolly along the camera vector toward target
   *    - Default speeds
   *
   *  Writes onto the active controls instance directly — we don't
   *  fight the kapsule's `enableNavigationControls` onChange
   *  handler, which writes `controls.enabled`, because these are
   *  per-axis flags and a separate concern. Setting both
   *  trackball-style (`noRotate`) and orbit-style (`enableRotate`)
   *  flags keeps the path safe regardless of which control type the
   *  host configured (we default to orbit; some hosts may not). */
  const applyControlsForMode = (m: GraphMode): void => {
    const controls = (instance.controls as (() => unknown | null) | undefined)?.();
    if (!controls) return;
    const c = controls as {
      noRotate?: boolean;
      enableRotate?: boolean;
      zoomToCursor?: boolean;
      screenSpacePanning?: boolean;
      zoomSpeed?: number;
      panSpeed?: number;
      mouseButtons?: { LEFT?: unknown; RIGHT?: unknown; MIDDLE?: unknown };
    };
    if (m === "2d") {
      c.noRotate = true; // trackball
      c.enableRotate = false; // orbit
      c.zoomToCursor = true; // orbit-only; trackball ignores
      c.screenSpacePanning = true;
      c.zoomSpeed = 1.6;
      c.panSpeed = 1.2;
      if (c.mouseButtons) c.mouseButtons.LEFT = MOUSE.PAN;
    } else {
      c.noRotate = false;
      c.enableRotate = true;
      c.zoomToCursor = false;
      c.screenSpacePanning = false;
      c.zoomSpeed = 1.0; // orbit defaults
      c.panSpeed = 1.0;
      if (c.mouseButtons) c.mouseButtons.LEFT = MOUSE.ROTATE;
    }
  };

  const pinNodesToPlane = (): void => {
    // fz = 0 holds each node on the xy plane. Use the underlying
    // graphData() getter so we see live (kapsule-mutated) refs, not
    // a stale snapshot.
    const data = instance.graphData() as GraphData<N, L>;
    for (const node of data.nodes) {
      (node as unknown as { fz: number }).fz = 0;
    }
  };

  const releaseNodesFromPlane = (): void => {
    const data = instance.graphData() as GraphData<N, L>;
    for (const node of data.nodes) {
      delete (node as unknown as Record<string, unknown>).fz;
    }
  };

  /** Seed each node with a randomised z offset so a 2D→3D transition
   *  has visible depth expansion immediately — d3-force's manyBody
   *  needs some z perturbation to push nodes apart along that axis,
   *  and starting all nodes at z=0 leaves the simulation sluggish.
   *  Scale is roughly proportional to the existing xy-spread so it
   *  feels balanced with the rest of the layout.
   *
   *  Returns the assigned z values so a tween can drive each node
   *  from 0 → the seeded target over the transition duration. */
  const seedDepthOffsets = (): Map<N, number> => {
    const data = instance.graphData() as GraphData<N, L>;
    const targets = new Map<N, number>();
    // Estimate the xy radius so the z-spread is proportional. Falls
    // back to a small default for nearly-empty graphs.
    let maxR = 0;
    for (const n of data.nodes) {
      const x = (n as unknown as { x?: number }).x ?? 0;
      const y = (n as unknown as { y?: number }).y ?? 0;
      const r = Math.hypot(x, y);
      if (r > maxR) maxR = r;
    }
    const spread = Math.max(40, maxR * 0.6);
    for (const n of data.nodes) {
      // Centred uniform random in [-spread, +spread]. Good enough for
      // visual diversity; the simulation refines from there.
      targets.set(n, (Math.random() - 0.5) * 2 * spread);
    }
    return targets;
  };

  // Initial-mode setup. No animation on first mount — just snap.
  // In both modes we explicitly set the camera before `graphData()`
  // so the kapsule's `cbrt(n)*170` auto-placement is bypassed (its
  // guard requires the camera to still be at its post-init default
  // of (0, 0, 1000); writing to `cameraPosition` defeats that). The
  // host's React-level `useEffect` runs the real auto-fit once the
  // first few simulation ticks have spread the nodes out — letting
  // the kapsule also move the camera would produce a visible double
  // hop.
  if (currentMode === "2d") {
    pinNodesToPlane();
    applyControlsForMode("2d");
    instance.cameraPosition?.(
      { x: 0, y: 0, z: TWO_D_CAMERA_DISTANCE },
      { x: 0, y: 0, z: 0 },
      0,
    );
  } else {
    applyControlsForMode("3d");
    instance.cameraPosition?.(
      { x: 0, y: 0, z: DEFAULT_3D_DISTANCE },
      { x: 0, y: 0, z: 0 },
      0,
    );
  }

  // ── GraphEngine implementation ────────────────────────────────

  const cameraPosition = (): Coords3D | undefined =>
    instance.cameraPosition?.() as Coords3D | undefined;

  const engine: UnifiedEngine<N, L> = {
    get mode() {
      return currentMode;
    },

    setGraphData(data: GraphData<N, L>) {
      instance.graphData(data);
      // Newly-added nodes need the same z-pin discipline as existing
      // ones when we're in 2D mode — otherwise they'd float off the
      // plane.
      if (currentMode === "2d") pinNodesToPlane();
    },
    getGraphData() {
      return instance.graphData() as GraphData<N, L>;
    },

    fit(durationMs?: number, padding?: number) {
      // We compute the fit ourselves rather than delegating to the
      // upstream `zoomToFit`, which (a) hardcodes the camera lookAt to
      // world-origin instead of the bbox centre, (b) uses
      // `atan(fov_rad)` where the correct formula is `tan(fov/2)` —
      // pulling the camera ~30–45 % further back than needed — and
      // (c) collapses the bbox to a single "max corner-to-origin"
      // scalar instead of fitting per-axis extent.
      cancelAnim?.();
      const dur = durationMs ?? 400;
      const pad = padding ?? 40;

      const bbox = instance.getGraphBbox?.() as
        | { x: [number, number]; y: [number, number]; z: [number, number] }
        | undefined;
      if (!bbox) return;

      const cameraObj = (instance.camera?.() as
        | { fov?: number; aspect?: number }
        | undefined) ?? {};
      const fov = cameraObj.fov ?? 50;
      const w = (instance.width?.() as number) ?? opts.width;
      const h = (instance.height?.() as number) ?? opts.height;
      if (w <= 0 || h <= 0) return;

      // Pixel padding → effective viewport. Floored to 1 so a
      // sub-padding-sized canvas doesn't produce a negative dimension
      // (and a negative distance ratio).
      const effW = Math.max(1, w - 2 * pad);
      const effH = Math.max(1, h - 2 * pad);
      const aspect = cameraObj.aspect ?? w / h;

      const center = {
        x: (bbox.x[0] + bbox.x[1]) / 2,
        y: (bbox.y[0] + bbox.y[1]) / 2,
        z: (bbox.z[0] + bbox.z[1]) / 2,
      };
      // Clamp tiny extents so a single-node graph (or a degenerate
      // bbox before any tick has run) doesn't collapse the fit
      // distance to a value smaller than a node's render radius.
      const halfX = Math.max((bbox.x[1] - bbox.x[0]) / 2, 1);
      const halfY = Math.max((bbox.y[1] - bbox.y[0]) / 2, 1);
      const halfZ = Math.max((bbox.z[1] - bbox.z[0]) / 2, 0);

      const halfFovV = ((fov * Math.PI) / 180) / 2;
      const tanHalfV = Math.tan(halfFovV);
      // Floor of 60 world units mirrors the link-focus fit floor —
      // far enough that a sub-100 unit graph doesn't end up with the
      // camera inside its node spheres.
      const MIN_DIST = 60;

      const cur = cameraPosition();
      if (!cur) return;

      let endPos: Coords3D;
      const endLookAt = { x: center.x, y: center.y, z: center.z };

      if (currentMode === "2d") {
        // Top-down: camera at (cx, cy, cz), looking at z = center.z
        // (which is 0 because nodes are pinned to fz=0 in this mode,
        // but we use center.z for symmetry). Half-height the camera
        // can see at the lookAt plane is `cz * tan(halfFovV)`; pixel
        // padding inflates the requested half-extents by viewport /
        // effective-viewport ratio.
        const reqDistV = ((halfY * h) / effH) / tanHalfV;
        const reqDistH = ((halfX * h) / effW) / tanHalfV;
        const dist = Math.max(reqDistV, reqDistH, MIN_DIST);
        endPos = { x: center.x, y: center.y, z: center.z + dist };
        cachedDistance = dist;
      } else {
        // 3D: keep the current viewing direction (so an orbited user
        // doesn't get yanked back to a canonical pose) and slide the
        // camera along that vector to a distance that fits a bounding
        // sphere around the bbox. Sphere fit is orientation-
        // independent — slightly conservative under axis-aligned
        // views but always correct.
        const radius = Math.hypot(halfX, halfY, halfZ);
        const halfFovH = Math.atan(tanHalfV * aspect);
        const reqDistV = (radius * h) / (effH * Math.sin(halfFovV));
        const reqDistH = (radius * w) / (effW * Math.sin(halfFovH));
        const dist = Math.max(reqDistV, reqDistH, MIN_DIST);

        const curLookAt = cur.lookAt ?? { x: 0, y: 0, z: 0 };
        const dx = cur.x - curLookAt.x;
        const dy = cur.y - curLookAt.y;
        const dz = cur.z - curLookAt.z;
        const mag = Math.hypot(dx, dy, dz);
        const unit =
          mag === 0
            ? { x: 0, y: 0, z: 1 }
            : { x: dx / mag, y: dy / mag, z: dz / mag };
        endPos = {
          x: center.x + unit.x * dist,
          y: center.y + unit.y * dist,
          z: center.z + unit.z * dist,
        };
        cachedDistance = dist;
      }

      if (dur <= 0) {
        instance.cameraPosition?.(endPos, endLookAt, 0);
        return;
      }

      const startLookAt = cur.lookAt ?? { x: 0, y: 0, z: 0 };
      cancelAnim = runAnim(dur, (t) => {
        instance.cameraPosition?.(
          {
            x: lerp(cur.x, endPos.x, t),
            y: lerp(cur.y, endPos.y, t),
            z: lerp(cur.z, endPos.z, t),
          },
          {
            x: lerp(startLookAt.x, endLookAt.x, t),
            y: lerp(startLookAt.y, endLookAt.y, t),
            z: lerp(startLookAt.z, endLookAt.z, t),
          },
          0,
        );
      });
    },

    centerAt(x: number, y: number, z?: number, durationMs?: number) {
      if (currentMode === "2d") {
        // 2D: only the xy components matter for the lookAt; keep the
        // camera locked directly above the new centre at z=
        // TWO_D_CAMERA_DISTANCE.
        instance.cameraPosition?.(
          { x, y, z: TWO_D_CAMERA_DISTANCE },
          { x, y, z: 0 },
          durationMs ?? 0,
        );
        return;
      }
      // 3D: preserve current lookAt so the orbit isn't yanked.
      const cur = cameraPosition();
      instance.cameraPosition?.(
        { x, y, z: z ?? cachedDistance },
        cur?.lookAt,
        durationMs ?? 0,
      );
    },

    zoom(scale: number, durationMs?: number) {
      const cur = cameraPosition();
      if (!cur) return;
      const distance = Math.hypot(cur.x, cur.y, cur.z);
      cachedDistance = distance / Math.max(scale, 0.001);
      // Move the camera along its current direction-from-origin
      // vector, scaled to the new distance. Works in both modes
      // because the 2D top-down vector is just (0, 0, 1).
      const unit =
        distance === 0
          ? { x: 0, y: 0, z: 1 }
          : { x: cur.x / distance, y: cur.y / distance, z: cur.z / distance };
      instance.cameraPosition?.(
        {
          x: unit.x * cachedDistance,
          y: unit.y * cachedDistance,
          z: unit.z * cachedDistance,
        },
        cur.lookAt,
        durationMs ?? 0,
      );
    },

    getZoom() {
      const cur = cameraPosition();
      if (!cur) return 1;
      const d = Math.hypot(cur.x, cur.y, cur.z);
      return d === 0 ? 1 : cachedDistance / d;
    },

    screen2Graph(x: number, y: number, distance?: number) {
      const out = instance.screen2GraphCoords?.(x, y, distance ?? 0) as
        | { x: number; y: number; z: number }
        | undefined;
      return out ?? { x, y, z: 0 };
    },

    graph2Screen(x: number, y: number, z?: number) {
      const out = instance.graph2ScreenCoords?.(x, y, z ?? 0) as
        | { x: number; y: number; z: number }
        | undefined;
      return out ?? { x, y, z: z ?? 0 };
    },

    getGraphBbox() {
      const bbox = instance.getGraphBbox?.() as
        | { x: [number, number]; y: [number, number]; z: [number, number] }
        | undefined;
      return bbox ?? { x: [0, 0], y: [0, 0], z: [0, 0] };
    },

    pause() {
      instance.pauseAnimation?.();
    },
    resume() {
      instance.resumeAnimation?.();
    },
    reheat() {
      instance.d3ReheatSimulation?.();
    },
    resize(width: number, height: number) {
      instance.width!(width);
      instance.height!(height);
    },
    d3Force(name: string, fn?: unknown | null) {
      const setter = instance.d3Force;
      if (typeof setter !== "function") return undefined;
      if (fn === undefined) return setter.call(instance, name);
      return setter.call(instance, name, fn);
    },
    emitParticle(link: L) {
      instance.emitParticle?.(link as unknown);
    },

    stopAnimation() {
      cancelAnim?.();
      cancelAnim = null;
    },

    focusOn(target, focusOpts) {
      cancelAnim?.();
      const cur = cameraPosition();
      if (!cur) return;
      const tz = target.z ?? 0;
      const dur = focusOpts?.durationMs ?? 1000;

      if (currentMode === "2d") {
        // 2D focus: pan top-down toward (target.x, target.y) and
        // optionally retighten the zoom.
        const endZ =
          focusOpts?.zoom !== undefined
            ? cachedDistance / Math.max(focusOpts.zoom, 0.001)
            : TWO_D_CAMERA_DISTANCE;
        cancelAnim = runAnim(dur, (t) => {
          const x = lerp(cur.x, target.x, t);
          const y = lerp(cur.y, target.y, t);
          const z = lerp(cur.z, endZ, t);
          instance.cameraPosition?.(
            { x, y, z },
            { x, y, z: 0 },
            0,
          );
        });
        return;
      }

      // 3D focus: keep current viewing direction, place camera at
      // `distance` along that vector from the target.
      const dx = cur.x - target.x;
      const dy = cur.y - target.y;
      const dz = cur.z - tz;
      const mag = Math.hypot(dx, dy, dz);
      const distance = focusOpts?.distance ?? 120;
      const u =
        mag === 0
          ? { x: 0, y: 0, z: 1 }
          : { x: dx / mag, y: dy / mag, z: dz / mag };
      const endPos = {
        x: target.x + u.x * distance,
        y: target.y + u.y * distance,
        z: tz + u.z * distance,
      };
      const endLookAt = { x: target.x, y: target.y, z: tz };
      const startLookAt = cur.lookAt ?? { x: 0, y: 0, z: 0 };
      cancelAnim = runAnim(dur, (t) => {
        instance.cameraPosition?.(
          {
            x: lerp(cur.x, endPos.x, t),
            y: lerp(cur.y, endPos.y, t),
            z: lerp(cur.z, endPos.z, t),
          },
          {
            x: lerp(startLookAt.x, endLookAt.x, t),
            y: lerp(startLookAt.y, endLookAt.y, t),
            z: lerp(startLookAt.z, endLookAt.z, t),
          },
          0,
        );
      });
    },

    getCameraState(): CameraState {
      const cur = cameraPosition() ?? { x: 0, y: 0, z: DEFAULT_3D_DISTANCE };
      if (currentMode === "2d") {
        // Project the 3D camera state down to the 2D pan-zoom shape
        // used by the old Canvas2D engine. `k` (zoom) is derived from
        // distance the same way `getZoom()` does it.
        const lookAt = cur.lookAt ?? { x: 0, y: 0, z: 0 };
        const k = cachedDistance / Math.max(Math.hypot(cur.x, cur.y, cur.z), 1);
        return { mode: "2d", x: lookAt.x, y: lookAt.y, k };
      }
      return {
        mode: "3d",
        x: cur.x,
        y: cur.y,
        z: cur.z,
        lookAt: cur.lookAt ?? { x: 0, y: 0, z: 0 },
      };
    },

    setCameraState(state, durationMs) {
      cancelAnim?.();
      const dur = durationMs ?? 0;
      const cur = cameraPosition() ?? { x: 0, y: 0, z: DEFAULT_3D_DISTANCE };

      if (state.mode === "2d") {
        // Map (x, y, k) back into 3D top-down camera coordinates.
        const targetZ =
          k2Distance(state.k) ?? TWO_D_CAMERA_DISTANCE;
        if (dur <= 0) {
          instance.cameraPosition?.(
            { x: state.x, y: state.y, z: targetZ },
            { x: state.x, y: state.y, z: 0 },
            0,
          );
          return;
        }
        const startLookAt = cur.lookAt ?? { x: 0, y: 0, z: 0 };
        cancelAnim = runAnim(dur, (t) => {
          const x = lerp(cur.x, state.x, t);
          const y = lerp(cur.y, state.y, t);
          const z = lerp(cur.z, targetZ, t);
          instance.cameraPosition?.(
            { x, y, z },
            {
              x: lerp(startLookAt.x, state.x, t),
              y: lerp(startLookAt.y, state.y, t),
              z: 0,
            },
            0,
          );
        });
        return;
      }

      // 3D shape restore — full camera + lookAt.
      if (dur <= 0) {
        instance.cameraPosition?.(
          { x: state.x, y: state.y, z: state.z },
          state.lookAt,
          0,
        );
        return;
      }
      const startLookAt = cur.lookAt ?? state.lookAt;
      cancelAnim = runAnim(dur, (t) => {
        instance.cameraPosition?.(
          {
            x: lerp(cur.x, state.x, t),
            y: lerp(cur.y, state.y, t),
            z: lerp(cur.z, state.z, t),
          },
          {
            x: lerp(startLookAt.x, state.lookAt.x, t),
            y: lerp(startLookAt.y, state.lookAt.y, t),
            z: lerp(startLookAt.z, state.lookAt.z, t),
          },
          0,
        );
      });
    },

    applyProps(props, prev) {
      // Diff + apply via the 3D binding table for both modes — the
      // underlying engine is always the 3D kapsule, so the 3D-only
      // props (nodeOpacity, linkResolution, …) are valid throughout.
      applyDiffedProps(
        instance as unknown as Record<string, (value: unknown) => unknown>,
        props as unknown as LoraGraphCanvasProps<NodeObject, LinkObject>,
        prev as unknown as LoraGraphCanvasProps<NodeObject, LinkObject>,
        "3d",
      );
    },

    getCanvasElement() {
      return mount.querySelector("canvas") as HTMLCanvasElement | null;
    },

    destroy() {
      cancelAnim?.();
      cancelAnim = null;
      instance._destructor();
    },

    // ── Unified-specific: smooth mode transition ─────────────────
    setMode(target: GraphMode, durationMs: number = 800) {
      if (target === currentMode) return;
      const fromMode = currentMode;
      currentMode = target;

      cancelAnim?.();

      const cur = cameraPosition() ?? { x: 0, y: 0, z: DEFAULT_3D_DISTANCE };

      if (target === "2d") {
        // 3D → 2D: tween each node's z toward 0, tween camera from
        // wherever it sits to the top-down anchor, then snap fz=0
        // and disable orbit controls when the tween finishes.
        last3DCamera = {
          x: cur.x,
          y: cur.y,
          z: cur.z,
          lookAt: cur.lookAt ?? { x: 0, y: 0, z: 0 },
        };
        const data = instance.graphData() as GraphData<N, L>;
        // Capture each node's starting z so we can lerp it down.
        const starts = new Map<N, number>();
        for (const n of data.nodes) {
          const z = (n as unknown as { z?: number }).z;
          starts.set(n, typeof z === "number" ? z : 0);
        }
        const lookAt = cur.lookAt ?? { x: 0, y: 0, z: 0 };
        const endLookAt = { x: lookAt.x, y: lookAt.y, z: 0 };
        // Preserve the user's effective zoom level across the mode
        // switch: the top-down end-distance is whatever distance they
        // were already at in 3D, not the canonical
        // TWO_D_CAMERA_DISTANCE. Otherwise zoomed-in 3D users see a
        // big dolly-out (and zoomed-out users see a dolly-in) on top
        // of the perspective rotation — which is exactly the "flashy
        // switch" feel we want to avoid. Floor of 1 just guards
        // against a degenerate start (camera exactly at lookAt).
        const distToLookAt =
          Math.hypot(cur.x - lookAt.x, cur.y - lookAt.y, cur.z - lookAt.z) ||
          TWO_D_CAMERA_DISTANCE;
        const endCamera = {
          x: lookAt.x,
          y: lookAt.y,
          z: lookAt.z + distToLookAt,
        };

        cancelAnim = runAnim(
          durationMs,
          (t) => {
            // Nodes: drag z toward 0 along an eased path. Direct
            // mutation of node.z bypasses the simulation for this frame,
            // which is what we want — once the tween ends, fz=0
            // permanently locks them.
            for (const [node, startZ] of starts) {
              (node as unknown as { z: number }).z = lerp(startZ, 0, t);
            }
            // Camera: top-down approach.
            instance.cameraPosition?.(
              {
                x: lerp(cur.x, endCamera.x, t),
                y: lerp(cur.y, endCamera.y, t),
                z: lerp(cur.z, endCamera.z, t),
              },
              {
                x: lerp(lookAt.x, endLookAt.x, t),
                y: lerp(lookAt.y, endLookAt.y, t),
                z: lerp(lookAt.z, endLookAt.z, t),
              },
              0,
            );
            if (t >= 1) {
              // Finalise: pin every node and switch to 2D controls
              // (pan + zoom-to-cursor, rotation locked). Idempotent,
              // so safe to run inside the last tween frame's tick.
              pinNodesToPlane();
              applyControlsForMode("2d");
              cachedDistance = distToLookAt;
            }
          },
          undefined,
          easeInOutCubic,
        );
        return;
      }

      // 2D → 3D: release the fz pins, then drive each node from
      // z=0 toward a randomised target during the tween so the depth
      // expansion is *visible* rather than something the simulation
      // has to discover from scratch after the fact. After the tween
      // lands, we hand off to d3-force with a reheat — the
      // perturbation is enough for manyBody/collide to refine the
      // 3D layout naturally from there.
      releaseNodesFromPlane();
      applyControlsForMode("3d");
      void fromMode; // (kept for symmetry; not used)

      const depthTargets = seedDepthOffsets();
      const depthStarts = new Map<N, number>();
      for (const [n] of depthTargets) {
        const z = (n as unknown as { z?: number }).z;
        depthStarts.set(n, typeof z === "number" ? z : 0);
      }

      const startLookAt = cur.lookAt ?? { x: 0, y: 0, z: 0 };
      // Default 3D camera: when the user has no remembered 3D pose
      // (first-time 2D → 3D), construct one by *rotating* their
      // current view rather than snapping to a canonical
      // DEFAULT_3D_DISTANCE position. Same lookAt, same distance —
      // we only introduce a ~30° tilt off the z-axis so depth
      // becomes visible. Without this the camera flies in from
      // straight-down 1200 to (0,0,300), which is the jarring jump
      // the user wants to eliminate. The y-axis is the natural tilt
      // axis (camera moves "south" of the lookAt and looks slightly
      // up-and-forward) — orbit controls happily take over from any
      // start orientation once the tween ends.
      const dxStart = cur.x - startLookAt.x;
      const dyStart = cur.y - startLookAt.y;
      const dzStart = cur.z - startLookAt.z;
      const distFromLookAt =
        Math.hypot(dxStart, dyStart, dzStart) || DEFAULT_3D_DISTANCE;
      const tiltAngle = Math.PI / 6; // 30°
      const endCamera = last3DCamera ?? {
        x: startLookAt.x,
        y: startLookAt.y - distFromLookAt * Math.sin(tiltAngle),
        z: startLookAt.z + distFromLookAt * Math.cos(tiltAngle),
        lookAt: { ...startLookAt },
      };
      const endLookAt = endCamera.lookAt ?? { x: 0, y: 0, z: 0 };
      cancelAnim = runAnim(
        durationMs,
        (t) => {
          // Drive each node's z from its starting value (typically 0
          // after a 2D session) toward the seeded depth target. This
          // is in lockstep with the camera tween so the user sees a
          // single coherent "expanding into depth" motion.
          for (const [node, target] of depthTargets) {
            const start = depthStarts.get(node) ?? 0;
            (node as unknown as { z: number }).z = lerp(start, target, t);
          }
          instance.cameraPosition?.(
            {
              x: lerp(cur.x, endCamera.x, t),
              y: lerp(cur.y, endCamera.y, t),
              z: lerp(cur.z, endCamera.z, t),
            },
            {
              x: lerp(startLookAt.x, endLookAt.x, t),
              y: lerp(startLookAt.y, endLookAt.y, t),
              z: lerp(startLookAt.z, endLookAt.z, t),
            },
            0,
          );
          if (t >= 1) {
            // Hand off to the force simulation. Reheat fires AFTER the
            // seeded z values are in place, so the alpha decay refines
            // from the perturbed positions instead of fighting an
            // exact-zero start that the depth axis was never going to
            // escape on its own.
            instance.d3ReheatSimulation?.();
            cachedDistance = Math.hypot(
              endCamera.x - endLookAt.x,
              endCamera.y - endLookAt.y,
              endCamera.z - endLookAt.z,
            );
          }
        },
        undefined,
        easeInOutCubic,
      );
    },
  };

  return engine;

  // Map a 2D `k` (zoom factor) back to the perspective-camera
  // distance it implies. `getZoom()` defines `k = cachedDistance /
  // d`, so `d = cachedDistance / k`.
  function k2Distance(k: number): number | null {
    if (!Number.isFinite(k) || k === 0) return null;
    return cachedDistance / k;
  }
}

export type EngineUnified = ReturnType<typeof createEngineUnified>;
