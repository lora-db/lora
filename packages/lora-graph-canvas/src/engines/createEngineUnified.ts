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
import { runAnim, runFollow, lerp, easeInOutCubic } from "./rafAnim";
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

/** Shared frozen object used as a default lookAt when the camera
 *  hasn't been told to look anywhere specific. Avoids allocating
 *  `{ x: 0, y: 0, z: 0 }` literals in hot paths. */
const ORIGIN_LOOKAT = Object.freeze({ x: 0, y: 0, z: 0 });

export interface UnifiedEngine<
  N extends NodeObject = NodeObject,
  L extends LinkObject = LinkObject,
> extends GraphEngine<N, L> {
  /** Switch presentation mode in place. Animates the camera + the
   *  per-node z constraints; does not destroy the engine. */
  setMode(mode: GraphMode, durationMs?: number): void;

  /** First-load reveal that runs concurrently with the force
   *  simulation: every animation frame, recompute the bbox-fitted
   *  pose and ease the camera toward it via a critically-damped
   *  spring. The spring's time constant is tuned by node count
   *  (small graphs settle in ~200 ms; large graphs ease over ~900 ms
   *  so we don't over-react to early ticks where the bbox is still
   *  exploding outward). Cancels on user interaction, on
   *  `onEngineStop`, or after `maxDurationMs`. Padding is in CSS
   *  pixels and reserved on every side of the viewport.
   *
   *  Use on initial mount only — calling this on a graph the user
   *  has already explored will yank their camera. */
  introFollow(opts?: {
    padding?: number;
    maxDurationMs?: number;
    /** Optional time-constant override (seconds). If omitted, derived
     *  from node count via a perf-tier-style log scale. */
    tauSeconds?: number;
  }): void;
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

  // Internal one-shot subscribers fired alongside the user-facing
  // `onEngineStop` prop. The intro-reveal logic uses this to know when
  // the simulation has stabilised without clobbering the host's own
  // onEngineStop callback. Subscribers are responsible for removing
  // themselves after firing.
  const engineStopSubscribers = new Set<() => void>();

  // Wire event handlers through the trampoline so latest React props
  // always win without re-binding on every render.
  for (const name of EVENT_BINDINGS) {
    const setter = instance[name as keyof Kapsule3D];
    if (typeof setter !== "function") continue;
    if (name === "onEngineStop") {
      // Fan out to internal subscribers first so engine-owned logic
      // (e.g. introFollow) settles before a user handler gets a
      // chance to move the camera.
      setter.call(instance, (...args: unknown[]) => {
        for (const sub of [...engineStopSubscribers]) {
          try {
            sub();
          } catch {
            /* swallow — a buggy subscriber shouldn't break the host */
          }
        }
        const fn = handlerRef.current.onEngineStop;
        if (typeof fn === "function") (fn as (...a: unknown[]) => void)(...args);
      });
      continue;
    }
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

  // ── Layout-preserving graphData writes ────────────────────────
  // three-forcegraph hard-codes `.stop().alpha(1)` inside its kapsule
  // update on every graphData write — so removing a single node
  // re-energises the d3 simulation to full and the whole layout
  // drifts toward a new equilibrium for the next ~hundreds of ticks.
  // Mesh removal itself is incremental (nodeDataMapper.digest), so
  // the only visible bug is the simulation kick.
  //
  // We can't suppress the reheat without forking the vendored lib,
  // so we make the reheat impotent: pin every surviving node's
  // (x, y, z) into (fx, fy, fz) right before the write, then release
  // the pins once the simulation has actually settled. While alpha
  // decays from 1, the pinned axes lock each node to its captured
  // position — d3-force only reads fx/fy/fz, so the tick can run
  // freely without moving anything visible.
  //
  // Per-axis sets so we don't disturb user pins:
  //   - tempPinnedX/Y/Z record the value WE set fx/fy/fz to. On
  //     release, if the live value still equals what we stored, we
  //     drop the pin; if a drag (`fixOnDrop`) has since rewritten
  //     it, we leave it. Tracking the value (not just membership) is
  //     what distinguishes "our temp pin" from "user's drag pin
  //     happened during the cooldown window."
  //   - Set on entries that were previously unpinned on that axis.
  //     A partially-pinned node (e.g. fx set, fy/fz not) gets the
  //     unpinned axes temp-pinned the same way.
  const tempPinnedX = new WeakMap<N, number>();
  const tempPinnedY = new WeakMap<N, number>();
  const tempPinnedZ = new WeakMap<N, number>();
  // Per-node z snapshot captured at 3D → 2D transition time. Used
  // by `seedDepthOffsets` on the reverse 2D → 3D switch so a round-
  // trip restores the user's actual depth layout rather than
  // shuffling everything to a fresh random spread. Random fallback
  // still runs for nodes that weren't around for the previous 3D
  // session (incremental data updates while the user was in 2D).
  const savedZByNode = new WeakMap<N, number>();
  // Set true while a release subscriber is registered on
  // engineStopSubscribers. Coalesces multiple writes inside one
  // cooldown into a single release pass.
  let pinReleaseScheduled = false;

  // Tracks the data reference that was last written through
  // `setGraphData`. If the incoming write matches we treat it as a
  // redundant no-op write — typically the React forward-data effect
  // firing right after the engine factory already applied initialData.
  // Without this guard the redundant write would pin every just-seeded
  // node at its first-frame position, and the d3-force reheat would
  // find nothing to spread.
  let lastWrittenData: GraphData<N, L> = opts.initialData;

  // Decide whether a `setGraphData` write looks like an incremental
  // update (host edited a few nodes inside an otherwise-settled layout)
  // or a wholesale replacement (host swapped in a brand-new dataset).
  // We pin existing nodes only on incremental updates — pinning on a
  // wholesale replace would freeze the fresh seed positions and
  // prevent the d3-force layout from spreading them. Threshold is
  // "≥ half of the OLD nodes still appear in the NEW data, by id":
  // generous enough that a one-node delete + add still counts as
  // incremental, strict enough that a query that returns a completely
  // different graph doesn't.
  const looksIncremental = (
    oldData: GraphData<N, L>,
    newData: GraphData<N, L>,
  ): boolean => {
    if (oldData.nodes.length === 0) return false;
    if (newData.nodes.length === 0) return false;
    const oldIds = new Set<string | number>();
    for (const n of oldData.nodes) oldIds.add(n.id);
    let shared = 0;
    for (const n of newData.nodes) {
      if (oldIds.has(n.id)) shared++;
    }
    return shared * 2 >= oldData.nodes.length;
  };

  const pinExistingNodesInPlace = (): void => {
    const data = instance.graphData() as GraphData<N, L>;
    for (const n of data.nodes) {
      const node = n as unknown as {
        x?: number; y?: number; z?: number;
        fx?: number; fy?: number; fz?: number;
      };
      if (node.fx === undefined && typeof node.x === "number") {
        node.fx = node.x;
        tempPinnedX.set(n, node.x);
      }
      if (node.fy === undefined && typeof node.y === "number") {
        node.fy = node.y;
        tempPinnedY.set(n, node.y);
      }
      if (node.fz === undefined && typeof node.z === "number") {
        node.fz = node.z;
        tempPinnedZ.set(n, node.z);
      }
    }
  };

  const releaseTempPins = (): void => {
    pinReleaseScheduled = false;
    engineStopSubscribers.delete(releaseTempPins);
    const data = instance.graphData() as GraphData<N, L>;
    for (const n of data.nodes) {
      // `delete` rather than `= undefined` because d3-force checks
      // `node.fx != null` to decide whether the axis is pinned — both
      // forms work at runtime, but `delete` keeps the property absent
      // (matching the pre-pin state) and satisfies the optional-type
      // contract under exactOptionalPropertyTypes.
      const node = n as unknown as Record<string, unknown>;
      const px = tempPinnedX.get(n);
      if (px !== undefined && node.fx === px) {
        delete node.fx;
      }
      tempPinnedX.delete(n);
      const py = tempPinnedY.get(n);
      if (py !== undefined && node.fy === py) {
        delete node.fy;
      }
      tempPinnedY.delete(n);
      const pz = tempPinnedZ.get(n);
      if (pz !== undefined) {
        // 2D mode invariant: fz=0 must always hold, regardless of
        // what we temp-pinned to. The kapsule's reheat doesn't move
        // z (pinNodesToPlane is reapplied after every write), so the
        // captured value will be 0 here in practice — but be explicit
        // so a future change to pinNodesToPlane semantics can't break
        // the invariant.
        if (currentMode === "3d" && node.fz === pz) {
          delete node.fz;
        }
      }
      tempPinnedZ.delete(n);
    }
  };

  const scheduleTempPinRelease = (): void => {
    if (pinReleaseScheduled) return;
    pinReleaseScheduled = true;
    engineStopSubscribers.add(releaseTempPins);
  };

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
      // Screen-space pan in 3D so right-drag moves the scene relative
      // to the current view direction — drag up moves the world UP
      // in the viewport, including a world-Y component whenever the
      // camera is tilted. This is OrbitControls' own default; setting
      // it to `false` (as we used to) restricted pan to the world XZ
      // plane and made elevating the view by mouse impossible.
      // Explicit Y-axis bindings (Shift+wheel, Shift+Arrow Up/Down,
      // PageUp/PageDown, Q/E) still exist for axis-aligned motion;
      // this just unlocks the analog path.
      c.screenSpacePanning = true;
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

  /** Seed each node with a z offset so a 2D → 3D transition has
   *  visible depth expansion. Restores the per-node z captured at
   *  the previous 3D → 2D transition when available — so a quick
   *  mode toggle returns the user to the exact same 3D layout
   *  rather than reshuffling the cloud. New nodes (no saved value)
   *  get a fresh random offset; the spread is roughly proportional
   *  to the current xy radius so it stays balanced with the layout.
   *  Without any perturbation d3-force's manyBody can't push nodes
   *  apart along z, so unseeded nodes would stay flat.
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
      const saved = savedZByNode.get(n);
      if (typeof saved === "number" && Number.isFinite(saved)) {
        targets.set(n, saved);
      } else {
        // Centred uniform random in [-spread, +spread]. Good enough for
        // visual diversity; the simulation refines from there.
        targets.set(n, (Math.random() - 0.5) * 2 * spread);
      }
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

  // Read the live orbit pivot from `controls.target`. This is the
  // authoritative rotation/pan anchor — what OrbitControls actually
  // rotates around and what `cameraPosition(pos, lookAt, 0)` writes
  // when its `lookAt` arg lands. We can't use the `cameraPosition()`
  // getter's `lookAt` here because three-render-objects synthesises
  // it from `cameraPos + forward * 1000`, which only coincides with
  // the pivot when the camera-to-target distance is exactly 1000.
  // For mode transitions where we need the pivot's true xy/z to set
  // up the next mode's anchor, the synthesised value drifts and
  // shifts the resulting orbit centre.
  const readOrbitTarget = (): { x: number; y: number; z: number } | null => {
    const controls = (instance.controls as (() => unknown | null) | undefined)?.();
    const target = (controls as { target?: { x: number; y: number; z: number } } | null)
      ?.target;
    if (!target) return null;
    return { x: target.x, y: target.y, z: target.z };
  };

  // Live xy centroid of the node cloud, derived from the simulation
  // bbox. Used as the mode-switch orbit anchor so rotation/pan in
  // the next mode is centered on the nodes regardless of where the
  // user had panned the previous mode's camera. Returns null when
  // the bbox is unavailable or degenerate (empty graph, zero-sized
  // canvas) — callers should fall back to the current pivot in that
  // case so we don't anchor to the world origin.
  const readBboxCenter = (): { x: number; y: number; z: number } | null => {
    const bbox = instance.getGraphBbox?.() as
      | { x: [number, number]; y: [number, number]; z: [number, number] }
      | undefined;
    if (!bbox) return null;
    const cx = (bbox.x[0] + bbox.x[1]) / 2;
    const cy = (bbox.y[0] + bbox.y[1]) / 2;
    const cz = (bbox.z[0] + bbox.z[1]) / 2;
    if (!Number.isFinite(cx) || !Number.isFinite(cy) || !Number.isFinite(cz)) {
      return null;
    }
    return { x: cx, y: cy, z: cz };
  };

  // Shared camera-fit math used by `fit()` and the follow-fit
  // (`introFollow`). Reads the live bbox / camera state and writes
  // the target camera pose + bbox-centre lookAt into the supplied
  // `out` buffer — returns `out` on success, `null` if the engine
  // isn't ready (zero-sized canvas, no nodes, etc.).
  //
  // We compute the fit ourselves rather than delegating to the
  // upstream `zoomToFit`, which (a) hardcodes the camera lookAt to
  // world-origin instead of the bbox centre, (b) uses
  // `atan(fov_rad)` where the correct formula is `tan(fov/2)` —
  // pulling the camera ~30–45 % further back than needed — and
  // (c) collapses the bbox to a single "max corner-to-origin"
  // scalar instead of fitting per-axis extent.
  //
  // `inv` lets callers pre-fetch the viewport/fov constants once
  // (they don't change between simulation ticks, only on canvas
  // resize) and reuse them across calls — saves a few µs per frame
  // inside the introFollow loop. Pass `null` and the function will
  // read them itself.
  //
  // Side effect: writes `cachedDistance` so `getZoom()` reports the
  // implied zoom factor relative to the freshly-computed fit.
  const FIT_MIN_DIST = 60;
  interface FitPose {
    endPos: Coords3D;
    endLookAt: Coords3D;
    dist: number;
  }
  interface ViewportInv {
    fov: number;
    aspect: number;
    w: number;
    h: number;
    effW: number;
    effH: number;
    halfFovV: number;
    tanHalfV: number;
    sinHalfV: number;
    sinHalfH: number;
  }
  const readViewportInv = (padding: number): ViewportInv | null => {
    const cameraObj = (instance.camera?.() as
      | { fov?: number; aspect?: number }
      | undefined) ?? {};
    const fov = cameraObj.fov ?? 50;
    const w = (instance.width?.() as number) ?? opts.width;
    const h = (instance.height?.() as number) ?? opts.height;
    if (w <= 0 || h <= 0) return null;
    const effW = Math.max(1, w - 2 * padding);
    const effH = Math.max(1, h - 2 * padding);
    const aspect = cameraObj.aspect ?? w / h;
    const halfFovV = ((fov * Math.PI) / 180) / 2;
    const tanHalfV = Math.tan(halfFovV);
    const sinHalfV = Math.sin(halfFovV);
    const sinHalfH = Math.sin(Math.atan(tanHalfV * aspect));
    return { fov, aspect, w, h, effW, effH, halfFovV, tanHalfV, sinHalfV, sinHalfH };
  };

  const computeFitPose = (
    padding: number,
    out: FitPose,
    inv?: ViewportInv | null,
  ): FitPose | null => {
    const bbox = instance.getGraphBbox?.() as
      | { x: [number, number]; y: [number, number]; z: [number, number] }
      | undefined;
    if (!bbox) return null;

    const v = inv ?? readViewportInv(padding);
    if (!v) return null;

    const centerX = (bbox.x[0] + bbox.x[1]) / 2;
    const centerY = (bbox.y[0] + bbox.y[1]) / 2;
    const centerZ = (bbox.z[0] + bbox.z[1]) / 2;
    // Clamp tiny extents so a single-node graph (or a degenerate
    // bbox before any tick has run) doesn't collapse the fit
    // distance to a value smaller than a node's render radius.
    const halfX = Math.max((bbox.x[1] - bbox.x[0]) / 2, 1);
    const halfY = Math.max((bbox.y[1] - bbox.y[0]) / 2, 1);
    const halfZ = Math.max((bbox.z[1] - bbox.z[0]) / 2, 0);

    const cur = cameraPosition();
    if (!cur) return null;

    out.endLookAt.x = centerX;
    out.endLookAt.y = centerY;
    out.endLookAt.z = centerZ;
    let dist: number;

    if (currentMode === "2d") {
      // Top-down: camera at (cx, cy, cz + dist), looking at the
      // bbox centre. Half-height the camera can see at the lookAt
      // plane is `dist * tan(halfFovV)`; pixel padding inflates the
      // requested half-extents by viewport / effective-viewport
      // ratio.
      const reqDistV = ((halfY * v.h) / v.effH) / v.tanHalfV;
      const reqDistH = ((halfX * v.h) / v.effW) / v.tanHalfV;
      dist = reqDistV > reqDistH ? reqDistV : reqDistH;
      if (dist < FIT_MIN_DIST) dist = FIT_MIN_DIST;
      out.endPos.x = centerX;
      out.endPos.y = centerY;
      out.endPos.z = centerZ + dist;
    } else {
      // 3D: keep the current viewing direction (so an orbited user
      // doesn't get yanked back to a canonical pose) and slide the
      // camera along that vector to a distance that fits a bounding
      // sphere around the bbox. Sphere fit is orientation-
      // independent — slightly conservative under axis-aligned
      // views but always correct.
      const radius = Math.sqrt(halfX * halfX + halfY * halfY + halfZ * halfZ);
      const reqDistV = (radius * v.h) / (v.effH * v.sinHalfV);
      const reqDistH = (radius * v.w) / (v.effW * v.sinHalfH);
      dist = reqDistV > reqDistH ? reqDistV : reqDistH;
      if (dist < FIT_MIN_DIST) dist = FIT_MIN_DIST;

      const curLookAt = cur.lookAt ?? ORIGIN_LOOKAT;
      const dx = cur.x - curLookAt.x;
      const dy = cur.y - curLookAt.y;
      const dz = cur.z - curLookAt.z;
      const mag = Math.sqrt(dx * dx + dy * dy + dz * dz);
      const ux = mag === 0 ? 0 : dx / mag;
      const uy = mag === 0 ? 0 : dy / mag;
      const uz = mag === 0 ? 1 : dz / mag;
      out.endPos.x = centerX + ux * dist;
      out.endPos.y = centerY + uy * dist;
      out.endPos.z = centerZ + uz * dist;
    }
    out.dist = dist;
    cachedDistance = dist;
    return out;
  };

  const engine: UnifiedEngine<N, L> = {
    get mode() {
      return currentMode;
    },

    setGraphData(data: GraphData<N, L>) {
      // No-op when the same reference is being written twice in a row
      // — this is the common "React forward-data effect fires right
      // after the factory already wrote initialData" case. Writing
      // again would pin every just-seeded node and freeze the
      // simulation before it spreads them.
      if (data === lastWrittenData) return;
      const prevData = instance.graphData() as GraphData<N, L>;
      lastWrittenData = data;

      // Pin every existing node at its current position BEFORE handing
      // the new data to the kapsule — three-forcegraph reheats the d3
      // simulation to alpha=1 on every graphData write, and without
      // pins the surviving nodes would drift to a new equilibrium for
      // the entire cooldown. Pins make the reheat visually a no-op:
      // mesh removal still happens incrementally, but the layout stays
      // put. `releaseTempPins` fires on engineStop (alpha ≈ 0) and
      // restores the pre-write pin state for any axes we touched, so
      // future graphData writes still have a settled layout to grip
      // and user-driven pins (`fixOnDrop`) survive the round-trip.
      //
      // Only pin on incremental updates — a wholesale replace (new
      // dataset, mostly disjoint node ids) should let the fresh
      // layout find its own equilibrium. Otherwise the random seed
      // positions get frozen and the graph looks "stuck" on first
      // load / on a new query result.
      const incremental = looksIncremental(prevData, data);
      if (incremental) {
        pinExistingNodesInPlace();
      }
      instance.graphData(data);
      // Newly-added nodes need the same z-pin discipline as existing
      // ones when we're in 2D mode — otherwise they'd float off the
      // plane.
      if (currentMode === "2d") pinNodesToPlane();
      if (incremental) scheduleTempPinRelease();
    },
    getGraphData() {
      return instance.graphData() as GraphData<N, L>;
    },

    fit(durationMs?: number, padding?: number) {
      cancelAnim?.();
      const dur = durationMs ?? 400;
      const poseBuf: FitPose = {
        endPos: { x: 0, y: 0, z: 0 },
        endLookAt: { x: 0, y: 0, z: 0 },
        dist: 0,
      };
      const pose = computeFitPose(padding ?? 40, poseBuf);
      if (!pose) return;
      const cur = cameraPosition();
      if (!cur) return;

      if (dur <= 0) {
        instance.cameraPosition?.(pose.endPos, pose.endLookAt, 0);
        return;
      }

      const startLookAt = cur.lookAt ?? ORIGIN_LOOKAT;
      const endPos = pose.endPos;
      const endLookAt = pose.endLookAt;
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

    panBy(delta, durationMs) {
      const dx = delta.x ?? 0;
      const dy = delta.y ?? 0;
      const dz = currentMode === "2d" ? 0 : (delta.z ?? 0);
      if (dx === 0 && dy === 0 && dz === 0) return;
      cancelAnim?.();
      const cur = cameraPosition();
      if (!cur) return;
      const lookAt = cur.lookAt ?? { x: 0, y: 0, z: 0 };
      const dur = durationMs ?? 0;
      const endCam = { x: cur.x + dx, y: cur.y + dy, z: cur.z + dz };
      const endLookAt = {
        x: lookAt.x + dx,
        y: lookAt.y + dy,
        z: lookAt.z + dz,
      };
      if (dur <= 0) {
        instance.cameraPosition?.(endCam, endLookAt, 0);
        return;
      }
      cancelAnim = runAnim(dur, (t) => {
        instance.cameraPosition?.(
          {
            x: lerp(cur.x, endCam.x, t),
            y: lerp(cur.y, endCam.y, t),
            z: lerp(cur.z, endCam.z, t),
          },
          {
            x: lerp(lookAt.x, endLookAt.x, t),
            y: lerp(lookAt.y, endLookAt.y, t),
            z: lerp(lookAt.z, endLookAt.z, t),
          },
          0,
        );
      });
    },

    goTo(target, goToOpts) {
      cancelAnim?.();
      const cur = cameraPosition();
      if (!cur) return;
      const tz = target.z ?? 0;
      const dur = goToOpts?.durationMs ?? 600;
      const lookAt = cur.lookAt ?? { x: 0, y: 0, z: 0 };
      // Translate camera by the same delta as lookAt → preserves the
      // viewing direction and distance.
      const dx = target.x - lookAt.x;
      const dy = target.y - lookAt.y;
      const dz = currentMode === "2d" ? 0 : tz - lookAt.z;
      const endCam = { x: cur.x + dx, y: cur.y + dy, z: cur.z + dz };
      const endLookAt = {
        x: target.x,
        y: target.y,
        z: currentMode === "2d" ? 0 : tz,
      };
      if (dur <= 0) {
        instance.cameraPosition?.(endCam, endLookAt, 0);
        return;
      }
      cancelAnim = runAnim(dur, (t) => {
        instance.cameraPosition?.(
          {
            x: lerp(cur.x, endCam.x, t),
            y: lerp(cur.y, endCam.y, t),
            z: lerp(cur.z, endCam.z, t),
          },
          {
            x: lerp(lookAt.x, endLookAt.x, t),
            y: lerp(lookAt.y, endLookAt.y, t),
            z: lerp(lookAt.z, endLookAt.z, t),
          },
          0,
        );
      });
    },

    fitToNodes(nodeIds, durationMs, padding) {
      const ids = nodeIds;
      if (ids.length === 0) {
        engine.fit(durationMs, padding);
        return;
      }
      const idSet = new Set<string | number>(ids);
      const data = instance.graphData() as GraphData<N, L>;
      // Build a bbox from the selected nodes only. Reuse the bbox
      // shape that `instance.getGraphBbox()` returns, then plug it
      // into the same camera math `fit()` uses.
      let xmin = Infinity, xmax = -Infinity;
      let ymin = Infinity, ymax = -Infinity;
      let zmin = Infinity, zmax = -Infinity;
      let count = 0;
      for (const n of data.nodes) {
        if (!idSet.has(n.id)) continue;
        const node = n as unknown as { x?: number; y?: number; z?: number };
        const x = node.x ?? 0;
        const y = node.y ?? 0;
        const z = node.z ?? 0;
        if (x < xmin) xmin = x;
        if (x > xmax) xmax = x;
        if (y < ymin) ymin = y;
        if (y > ymax) ymax = y;
        if (z < zmin) zmin = z;
        if (z > zmax) zmax = z;
        count++;
      }
      if (count === 0) {
        engine.fit(durationMs, padding);
        return;
      }
      // Temporarily swap the kapsule's bbox by overriding for one call.
      // Simpler: replicate `fit()`'s math here on the local bbox.
      const dur = durationMs ?? 400;
      const pad = padding ?? 40;
      const cameraObj = (instance.camera?.() as
        | { fov?: number; aspect?: number }
        | undefined) ?? {};
      const fov = cameraObj.fov ?? 50;
      const w = (instance.width?.() as number) ?? opts.width;
      const h = (instance.height?.() as number) ?? opts.height;
      if (w <= 0 || h <= 0) return;
      const effW = Math.max(1, w - 2 * pad);
      const effH = Math.max(1, h - 2 * pad);
      const aspect = cameraObj.aspect ?? w / h;
      const center = {
        x: (xmin + xmax) / 2,
        y: (ymin + ymax) / 2,
        z: (zmin + zmax) / 2,
      };
      const halfX = Math.max((xmax - xmin) / 2, 1);
      const halfY = Math.max((ymax - ymin) / 2, 1);
      const halfZ = Math.max((zmax - zmin) / 2, 0);
      const halfFovV = ((fov * Math.PI) / 180) / 2;
      const tanHalfV = Math.tan(halfFovV);
      const MIN_DIST = 60;
      const cur = cameraPosition();
      if (!cur) return;
      let endPos: Coords3D;
      const endLookAt = { x: center.x, y: center.y, z: center.z };
      if (currentMode === "2d") {
        const reqDistV = ((halfY * h) / effH) / tanHalfV;
        const reqDistH = ((halfX * h) / effW) / tanHalfV;
        const dist = Math.max(reqDistV, reqDistH, MIN_DIST);
        endPos = { x: center.x, y: center.y, z: center.z + dist };
        cachedDistance = dist;
      } else {
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
      cancelAnim?.();
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
        //
        // We read the orbit pivot from `controls.target` directly,
        // not from `cur.lookAt`. The latter is a forward-projection
        // at 1000 units (see readOrbitTarget) — when the user is at
        // any other zoom level the projection drifts off the real
        // pivot, which then miscomputes the zoom-distance
        // preservation.
        const pivot = readOrbitTarget() ?? cur.lookAt ?? ORIGIN_LOOKAT;
        // Anchor the resulting 2D view to the node cloud's xy
        // centroid, not the current orbit pivot. If the user had
        // rotated/panned the 3D view to look at a corner of the
        // graph (or empty space beyond it), restoring the 2D view
        // to that point would leave the nodes off-screen or off to
        // the side — the user just wants to see their graph from
        // the top. Falls back to the current pivot if the bbox is
        // unavailable (empty graph mid-load).
        const bbox = readBboxCenter();
        const anchor = bbox ?? { x: pivot.x, y: pivot.y, z: 0 };
        last3DCamera = {
          x: cur.x,
          y: cur.y,
          z: cur.z,
          lookAt: { x: pivot.x, y: pivot.y, z: pivot.z },
        };
        const data = instance.graphData() as GraphData<N, L>;
        // Capture each node's starting z so we can lerp it down,
        // and persist it into savedZByNode so the reverse switch
        // restores the same 3D layout instead of re-randomising.
        const starts = new Map<N, number>();
        for (const n of data.nodes) {
          const z = (n as unknown as { z?: number }).z;
          const startZ = typeof z === "number" ? z : 0;
          starts.set(n, startZ);
          savedZByNode.set(n, startZ);
        }
        const startLookAt = { x: pivot.x, y: pivot.y, z: pivot.z };
        const endLookAt = { x: anchor.x, y: anchor.y, z: 0 };
        // Preserve the user's effective zoom level across the mode
        // switch: the top-down end-distance is whatever distance they
        // were already at in 3D, not the canonical
        // TWO_D_CAMERA_DISTANCE. Otherwise zoomed-in 3D users see a
        // big dolly-out (and zoomed-out users see a dolly-in) on top
        // of the perspective rotation — which is exactly the "flashy
        // switch" feel we want to avoid. Floor of 1 just guards
        // against a degenerate start (camera exactly at pivot).
        const distToLookAt =
          Math.hypot(cur.x - pivot.x, cur.y - pivot.y, cur.z - pivot.z) ||
          TWO_D_CAMERA_DISTANCE;
        // End camera sits directly above the bbox-centred anchor at
        // the preserved zoom distance. Anchoring `z` to
        // `distToLookAt` (not `anchor.z + distToLookAt`) keeps the
        // camera-to-pivot distance identical across the switch —
        // otherwise a non-zero anchor.z would silently inflate or
        // compress the 2D zoom.
        const endCamera = {
          x: anchor.x,
          y: anchor.y,
          z: distToLookAt,
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
                x: lerp(startLookAt.x, endLookAt.x, t),
                y: lerp(startLookAt.y, endLookAt.y, t),
                z: lerp(startLookAt.z, endLookAt.z, t),
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

      // Use `controls.target` for the current orbit anchor rather
      // than the kapsule's `cur.lookAt` getter. The getter projects
      // the camera forward by 1000 units, which doesn't reflect any
      // panning the user did in 2D — OrbitControls' pan mutates
      // `controls.target` directly, but the projected lookAt only
      // sees the camera's xy. Pin z to 0 as a safety belt; in 2D
      // mode the pivot should already be on the node plane.
      const pivot = readOrbitTarget() ?? cur.lookAt ?? ORIGIN_LOOKAT;
      const startLookAt = { x: pivot.x, y: pivot.y, z: 0 };
      // Anchor the 3D orbit pivot to the node cloud's xy centroid.
      // Using the user's current 2D pan centre instead (as an earlier
      // iteration did) lands the pivot in empty space whenever the
      // user has panned away from the cloud — they then "orbit" in
      // open space with the nodes drifting around at a tangent.
      // Anchoring to the bbox guarantees rotation anchors *on the
      // nodes*. z is pinned to 0 to match the post-seed bbox z-centre
      // (uniform [-spread, +spread] → centre ≈ 0).
      const bboxCentre = readBboxCenter();
      const endLookAt = bboxCentre
        ? { x: bboxCentre.x, y: bboxCentre.y, z: 0 }
        : { ...startLookAt };
      // 3D zoom distance: derive from the user's 2D camera height
      // (cur.z), which is the only zoom signal in top-down mode.
      // Using `Math.hypot(cur - startLookAt)` (the old form) would
      // also include any xy offset the user had panned to — making
      // the cold-start 3D camera fly out proportional to their pan,
      // which feels broken when they pan and then switch.
      const distFromLookAt = Math.abs(cur.z) || DEFAULT_3D_DISTANCE;
      const tiltAngle = Math.PI / 6; // 30°
      // When we have a saved 3D pose, preserve its orientation
      // (camera-to-pivot vector) and translate the whole rig so the
      // pivot lands on the node centroid. Without this translation,
      // the saved pose's lookAt would override `endLookAt` and put
      // rotation back at the stale 3D anchor.
      let endCamera: Coords3D;
      if (last3DCamera) {
        const lastLookAt = last3DCamera.lookAt ?? ORIGIN_LOOKAT;
        endCamera = {
          x: endLookAt.x + (last3DCamera.x - lastLookAt.x),
          y: endLookAt.y + (last3DCamera.y - lastLookAt.y),
          z: endLookAt.z + (last3DCamera.z - lastLookAt.z),
        };
      } else {
        // First-time 2D → 3D: rotate from straight-down to a 30°
        // tilt around the pivot. y-axis is the natural tilt axis
        // (camera moves "south" of the lookAt and looks
        // up-and-forward); orbit controls happily take over from
        // any start orientation once the tween ends.
        endCamera = {
          x: endLookAt.x,
          y: endLookAt.y - distFromLookAt * Math.sin(tiltAngle),
          z: endLookAt.z + distFromLookAt * Math.cos(tiltAngle),
        };
      }
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

    introFollow(introOpts) {
      // Continuously-tracked fit that runs in parallel with the force
      // simulation rather than waiting for it to settle. Every frame
      // we (cheaply) recompute the bbox-fitted target pose and ease
      // the camera toward it via a critically-damped spring; both the
      // target (forces are still spreading) and the camera (chasing
      // it) converge together. The previous implementation parked
      // far-out, waited up to 2.5 s for `onEngineStop`, then ran a
      // 1.5 s tween — that meant ~3 s of dead time on small graphs
      // before any zoom motion was visible. This trades the wait for
      // an immediately-visible follow that lands at the same framing.
      //
      // Performance:
      //   - Viewport / fov constants are hoisted once (read again on
      //     window resize via a ResizeObserver shim — but for the
      //     ~1–4 s reveal window we just refresh them every 30 frames
      //     to keep the loop branchless).
      //   - Bbox traversal (the dominant cost; O(N) with Box3
      //     allocation per node inside three-forcegraph) is sampled
      //     on a node-count-aware cadence: every 2 frames at < 200
      //     nodes, every 4 at < 2 k, every 6 above. The spring step
      //     still runs every frame so visual motion is smooth.
      //   - Pose, position, and lookAt buffers are pre-allocated and
      //     mutated in place. Zero allocations inside the loop body.
      //   - Cached function refs (cameraPosSetter, getBbox) avoid
      //     `?.()` property lookups every frame.
      const padding = introOpts?.padding ?? 40;
      const maxDurationMs = introOpts?.maxDurationMs ?? 4000;

      const data = instance.graphData() as GraphData<N, L>;
      const nodeCount = data.nodes?.length ?? 0;
      if (nodeCount === 0) return;

      // Spring time constant. Tracks the perf tiers from
      // utils/perfTier.ts: small graphs settle fast so we want a
      // snappy follow (~180 ms); large graphs spread over many ticks
      // so we slow the follow to avoid over-reacting to early-frame
      // bbox excursions. ~180 ms for 1 node, ~360 ms for 100, ~540
      // ms for 10k.
      const tauSeconds =
        introOpts?.tauSeconds ??
        (0.18 + 0.09 * Math.log10(Math.max(1, nodeCount)));

      // Bbox sample cadence — bbox traversal is the dominant cost so
      // we throttle it harder on larger graphs. Spring steps still
      // run every frame.
      const bboxSampleStride =
        nodeCount < 200 ? 2 : nodeCount < 2000 ? 4 : 6;

      cancelAnim?.();

      // Hoisted function refs and pre-allocated buffers. Reused
      // every frame — `cameraPosition(pos, lookAt, 0)` reads x/y/z
      // out of the input objects and writes them onto the camera,
      // so the same buffer pair is safe to recycle.
      const cameraSet = instance.cameraPosition as
        | ((p: Coords3D, l: Coords3D, dur: number) => unknown)
        | undefined;
      const setCamera = cameraSet
        ? (cameraSet as (p: Coords3D, l: Coords3D, dur: number) => unknown).bind(instance)
        : null;

      const poseBuf: FitPose = {
        endPos: { x: 0, y: 0, z: 0 },
        endLookAt: { x: 0, y: 0, z: 0 },
        dist: 0,
      };
      const camOut: Coords3D = { x: 0, y: 0, z: 0 };
      const lookOut: Coords3D = { x: 0, y: 0, z: 0 };

      // Read viewport constants once. Re-read every ~30 frames in
      // case the canvas resized mid-reveal (cheap — no bbox work).
      let inv = readViewportInv(padding);

      // Seed once: snap to a fitted pose for the *current* (small,
      // just-spawned) bbox so the reveal starts from an already-
      // padded view instead of the kapsule's far-out default. This
      // is the "padding from frame one" requirement — the spring
      // takes it the rest of the way as the layout spreads.
      const seed = computeFitPose(padding, poseBuf, inv);
      if (seed && setCamera) {
        setCamera(seed.endPos, seed.endLookAt, 0);
      }

      // Mutable camera + lookAt state, driven by the spring. Floats
      // (not boxed) — V8 keeps these in registers / stack slots.
      let camX = seed?.endPos.x ?? 0;
      let camY = seed?.endPos.y ?? 0;
      let camZ = seed?.endPos.z ?? DEFAULT_3D_DISTANCE;
      let lookXv = seed?.endLookAt.x ?? 0;
      let lookYv = seed?.endLookAt.y ?? 0;
      let lookZv = seed?.endLookAt.z ?? 0;

      let elapsedMs = 0;
      let frame = 0;
      let haveTarget = seed !== null;

      // User-interaction cancel: any pointer/wheel on the canvas
      // means the user is taking the camera over, and we should
      // back off immediately.
      let userInteracted = false;
      const renderer = instance.renderer?.() as
        | { domElement?: HTMLElement }
        | undefined;
      const dom = renderer?.domElement;
      const onUserInteract = (): void => {
        userInteracted = true;
      };
      dom?.addEventListener("pointerdown", onUserInteract, { passive: true });
      dom?.addEventListener("wheel", onUserInteract, { passive: true });

      const cleanup = (): void => {
        engineStopSubscribers.delete(onStop);
        dom?.removeEventListener("pointerdown", onUserInteract);
        dom?.removeEventListener("wheel", onUserInteract);
      };

      const snapToTarget = (): void => {
        if (!haveTarget || !setCamera) return;
        setCamera(poseBuf.endPos, poseBuf.endLookAt, 0);
      };

      const onStop = (): void => {
        // Force simulation has settled — do one final exact fit so
        // the landing pose matches what `fit()` would give, but only
        // if we haven't already drifted within ε of the target.
        if (haveTarget) {
          const ep = poseBuf.endPos;
          const dx = camX - ep.x;
          const dy = camY - ep.y;
          const dz = camZ - ep.z;
          const drift2 = dx * dx + dy * dy + dz * dz;
          const eps = Math.max(0.5, cachedDistance * 0.001);
          if (drift2 > eps * eps) snapToTarget();
        }
        cleanup();
        cancelAnim = null;
      };
      engineStopSubscribers.add(onStop);

      cancelAnim = runFollow((dt) => {
        if (userInteracted) {
          cleanup();
          cancelAnim = null;
          return true;
        }
        elapsedMs += dt * 1000;
        if (elapsedMs >= maxDurationMs) {
          snapToTarget();
          cleanup();
          cancelAnim = null;
          return true;
        }

        // Refresh viewport invariants every ~30 frames in case the
        // canvas was resized. Cheap relative to bbox traversal.
        if ((frame & 31) === 0) {
          const next = readViewportInv(padding);
          if (next) inv = next;
        }

        // Recompute the bbox-derived target on the sample cadence.
        // Spring steps every frame; bbox sampling is the work we're
        // throttling.
        if (frame % bboxSampleStride === 0) {
          const next = computeFitPose(padding, poseBuf, inv);
          if (next) haveTarget = true;
        }
        frame++;
        if (!haveTarget || !setCamera) return false;

        // Critically-damped first-order step:
        //   x += (target - x) * (1 - exp(-dt / τ))
        // Equivalent to a one-pole low-pass; no overshoot even as
        // the target itself moves.
        const alpha = 1 - Math.exp(-dt / tauSeconds);
        const ep = poseBuf.endPos;
        const el = poseBuf.endLookAt;
        camX += (ep.x - camX) * alpha;
        camY += (ep.y - camY) * alpha;
        camZ += (ep.z - camZ) * alpha;
        lookXv += (el.x - lookXv) * alpha;
        lookYv += (el.y - lookYv) * alpha;
        lookZv += (el.z - lookZv) * alpha;

        camOut.x = camX;
        camOut.y = camY;
        camOut.z = camZ;
        lookOut.x = lookXv;
        lookOut.y = lookYv;
        lookOut.z = lookZv;
        setCamera(camOut, lookOut, 0);

        return undefined;
      });
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
