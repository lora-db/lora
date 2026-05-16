// Outer kapsule for the 3D force-graph renderer. Owns the container
// DOM, the renderObjs (camera + controls + raycaster + WebGL
// renderer), the underlying force-graph scene object (three-forcegraph)
// that draws nodes/links as Three.js meshes, the DragControls
// integration, and the per-frame rAF loop. Most props are pass-through
// forwarders to either the inner force-graph or the renderObjs
// kapsule.
//
// LORA: ported from `3d-force-graph` (MIT, © Vasco Asturiano). The
// inner `three-forcegraph` and `three-render-objects` kapsules
// remain as npm deps for now — they're the heavyweight renderer +
// camera layers and will be internalised in follow-up phases. This
// shell is a TS port with proper types where the kapsule pattern
// allows; chainable inner-kapsule calls go through `unknown` casts.

import {
  AmbientLight,
  DirectionalLight,
  REVISION,
  type Object3D,
  type WebGLRendererParameters,
} from "three";
import { DragControls as ThreeDragControls } from "three/examples/jsm/controls/DragControls.js";
import ThreeForceGraph from "three-forcegraph";
import ThreeRenderObjects from "three-render-objects";

import Kapsule, { type KapsuleClassCtor } from "../../internal/kapsule";
import accessorFn from "../../internal/accessor-fn";
import linkKapsule from "../../internal/kapsule-link";

import "./styles.css";

// Allow consumers to provide `window.THREE` instead of bundling.
// (Upstream behaviour preserved.)
const three: typeof globalThis & {
  THREE?: { AmbientLight: typeof AmbientLight; DirectionalLight: typeof DirectionalLight };
} = globalThis as unknown as typeof globalThis & {
  THREE?: { AmbientLight: typeof AmbientLight; DirectionalLight: typeof DirectionalLight };
};
const ThreeBundle = three.THREE
  ? three.THREE
  : { AmbientLight, DirectionalLight };

const CAMERA_DISTANCE2NODES_FACTOR = 170;

// ── Pass-through prop / method tables ─────────────────────────────

const bindFG = linkKapsule(
  "forceGraph",
  ThreeForceGraph as unknown as new () => Record<string, unknown>,
);
const FG_PROPS = [
  "jsonUrl",
  "graphData",
  "numDimensions",
  "dagMode",
  "dagLevelDistance",
  "dagNodeFilter",
  "onDagError",
  "nodeRelSize",
  "nodeId",
  "nodeVal",
  "nodeResolution",
  "nodeColor",
  "nodeAutoColorBy",
  "nodeOpacity",
  "nodeVisibility",
  "nodeThreeObject",
  "nodeThreeObjectExtend",
  "nodePositionUpdate",
  "linkSource",
  "linkTarget",
  "linkVisibility",
  "linkColor",
  "linkAutoColorBy",
  "linkOpacity",
  "linkWidth",
  "linkResolution",
  "linkCurvature",
  "linkCurveRotation",
  "linkMaterial",
  "linkThreeObject",
  "linkThreeObjectExtend",
  "linkPositionUpdate",
  "linkDirectionalArrowLength",
  "linkDirectionalArrowColor",
  "linkDirectionalArrowRelPos",
  "linkDirectionalArrowResolution",
  "linkDirectionalParticles",
  "linkDirectionalParticleSpeed",
  "linkDirectionalParticleOffset",
  "linkDirectionalParticleWidth",
  "linkDirectionalParticleColor",
  "linkDirectionalParticleResolution",
  "linkDirectionalParticleThreeObject",
  "forceEngine",
  "d3AlphaDecay",
  "d3VelocityDecay",
  "d3AlphaMin",
  "ngraphPhysics",
  "warmupTicks",
  "cooldownTicks",
  "cooldownTime",
  "onEngineTick",
  "onEngineStop",
] as const;
const FG_METHODS = [
  "refresh",
  "getGraphBbox",
  "d3Force",
  "d3ReheatSimulation",
  "emitParticle",
] as const;

const bindRO = linkKapsule(
  "renderObjs",
  ThreeRenderObjects as unknown as new () => Record<string, unknown>,
);
const RO_PROPS = [
  "width",
  "height",
  "backgroundColor",
  "showNavInfo",
  "enablePointerInteraction",
] as const;
const RO_DIRECT_METHODS = [
  "lights",
  "cameraPosition",
  "postProcessingComposer",
] as const;

const linkedProps: Record<string, unknown> = {};
for (const p of FG_PROPS) linkedProps[p] = bindFG.linkProp(p);
for (const p of RO_PROPS) linkedProps[p] = bindRO.linkProp(p);

const linkedMethods: Record<string, unknown> = {};
for (const m of FG_METHODS) linkedMethods[m] = bindFG.linkMethod(m);
for (const m of RO_DIRECT_METHODS) linkedMethods[m] = bindRO.linkMethod(m);
// Renamed for the public surface — internally renderObjs calls them
// getScreenCoords / getSceneCoords.
linkedMethods.graph2ScreenCoords = bindRO.linkMethod("getScreenCoords");
linkedMethods.screen2GraphCoords = bindRO.linkMethod("getSceneCoords");

// ── State / Kapsule definition ────────────────────────────────────

interface State {
  forceGraph: Record<string, (...args: unknown[]) => unknown> & {
    position?: { x: number; y: number; z: number };
    _destructor?: () => void;
  };
  renderObjs: Record<string, (...args: unknown[]) => unknown> & {
    (host: HTMLElement): unknown;
    _destructor: () => void;
  };
  container?: HTMLDivElement;
  animationFrameRequestId?: number | null;
  enablePointerInteraction?: boolean;
  enableNodeDrag?: boolean;
  enableNavigationControls?: boolean;
  showPointerCursor?: unknown;
  forceEngine?: string;
  graphData: { nodes: Array<Record<string, unknown>>; links: Array<Record<string, unknown>> };
  hoverObj?: GraphObj | null;
  _dragControls?: ThreeDragControls | undefined;
  lastSetCameraZ?: number;
  nodeLabel?: unknown;
  linkLabel?: unknown;
  onNodeDrag?: (n: unknown, t: unknown) => void;
  onNodeDragEnd?: (n: unknown, t: unknown) => void;
  // Plus the linked pass-through prop bag (dynamically populated).
  [k: string]: unknown;
}

const asState = (state: unknown): State => state as State;

interface GraphObj extends Object3D {
  __graphObjType: "node" | "link";
  __data: unknown;
}

/** Walk an Object3D up the parent chain until we find the marker
 *  property set by three-forcegraph on the per-node / per-link
 *  group. Returns null for anything that isn't part of the graph. */
function getGraphObj(object: Object3D | null | undefined): GraphObj | null {
  let obj: Object3D | null = object ?? null;
  while (obj && !Object.prototype.hasOwnProperty.call(obj, "__graphObjType")) {
    obj = obj.parent;
  }
  return obj as GraphObj | null;
}

export interface ForceGraph3DConfigOptions {
  controlType?: "trackball" | "orbit" | "fly";
  rendererConfig?: WebGLRendererParameters;
  /** Additional renderers (e.g. CSS2DRenderer for HTML overlays).
   *  Typed loosely — three.js no longer exports a base `Renderer`
   *  interface in newer revisions. */
  extraRenderers?: unknown[];
}

const ForceGraph3D: KapsuleClassCtor = Kapsule({
  props: {
    nodeLabel: { default: "name", triggerUpdate: false },
    linkLabel: { default: "name", triggerUpdate: false },
    linkHoverPrecision: {
      default: 1,
      onChange(p: unknown, state: unknown) {
        const s = asState(state);
        (s.renderObjs as unknown as { lineHoverPrecision: (n: unknown) => unknown }).lineHoverPrecision(p);
      },
      triggerUpdate: false,
    },
    enableNavigationControls: {
      default: true,
      onChange(enable: unknown, state: unknown) {
        const s = asState(state);
        const controls = (s.renderObjs.controls as () => unknown)() as
          | { enabled?: boolean; domElement?: HTMLElement }
          | null;
        if (controls) {
          controls.enabled = !!enable;
          // Synthesize a pointerup on re-enable so any stuck
          // press-state inside the controls instance clears out.
          if (enable && controls.domElement) {
            controls.domElement.dispatchEvent(new PointerEvent("pointerup"));
          }
        }
      },
      triggerUpdate: false,
    },
    enableNodeDrag: { default: true, triggerUpdate: false },
    onNodeDrag: { default: () => {}, triggerUpdate: false },
    onNodeDragEnd: { default: () => {}, triggerUpdate: false },
    onNodeClick: { triggerUpdate: false },
    onNodeRightClick: { triggerUpdate: false },
    onNodeHover: { triggerUpdate: false },
    onLinkClick: { triggerUpdate: false },
    onLinkRightClick: { triggerUpdate: false },
    onLinkHover: { triggerUpdate: false },
    onBackgroundClick: { triggerUpdate: false },
    onBackgroundRightClick: { triggerUpdate: false },
    showPointerCursor: { default: true, triggerUpdate: false },
    ...linkedProps,
  },

  methods: {
    zoomToFit(
      this: { getGraphBbox: (...args: unknown[]) => unknown },
      state: unknown,
      transitionDuration?: number,
      padding?: number,
      ...bboxArgs: unknown[]
    ) {
      const s = asState(state);
      (s.renderObjs as unknown as {
        fitToBbox: (b: unknown, d?: number, p?: number) => unknown;
      }).fitToBbox(
        this.getGraphBbox(...bboxArgs),
        transitionDuration,
        padding,
      );
      return this;
    },
    pauseAnimation(state: unknown) {
      const s = asState(state);
      if (s.animationFrameRequestId != null) {
        cancelAnimationFrame(s.animationFrameRequestId);
        s.animationFrameRequestId = null;
      }
      return this;
    },
    resumeAnimation(this: { _animationCycle: () => void }, state: unknown) {
      const s = asState(state);
      if (s.animationFrameRequestId == null) this._animationCycle();
      return this;
    },
    _animationCycle(
      this: { renderer: () => { domElement: HTMLElement }; _animationCycle: () => void },
      state: unknown,
    ) {
      const s = asState(state);
      if (s.enablePointerInteraction) {
        // Reset cursor in case DragControls left it set.
        this.renderer().domElement.style.cursor = "";
      }
      (s.forceGraph as unknown as { tickFrame: () => unknown }).tickFrame();
      (s.renderObjs as unknown as { tick: () => unknown }).tick();
      s.animationFrameRequestId = requestAnimationFrame(this._animationCycle);
    },
    scene(state: unknown) {
      return (asState(state).renderObjs.scene as () => unknown)();
    },
    camera(state: unknown) {
      return (asState(state).renderObjs.camera as () => unknown)();
    },
    renderer(state: unknown) {
      return (asState(state).renderObjs.renderer as () => unknown)();
    },
    controls(state: unknown) {
      return (asState(state).renderObjs.controls as () => unknown)();
    },
    _destructor(
      this: {
        pauseAnimation: () => unknown;
        graphData: (d: unknown) => unknown;
      },
      state: unknown,
    ) {
      const s = asState(state);
      this.pauseAnimation();
      this.graphData({ nodes: [], links: [] });
      s.forceGraph._destructor?.();
      s.renderObjs._destructor();
    },
    ...linkedMethods,
  },

  stateInit: (opts?: Record<string, unknown>) => {
    const o = (opts ?? {}) as ForceGraph3DConfigOptions;
    const forceGraph = new (ThreeForceGraph as unknown as new () => Record<
      string,
      (...args: unknown[]) => unknown
    >)();
    const renderObjs = (
      ThreeRenderObjects as unknown as (opts: ForceGraph3DConfigOptions) => Record<
        string,
        (...args: unknown[]) => unknown
      >
    )({
      ...(o.controlType ? { controlType: o.controlType } : {}),
      ...(o.rendererConfig ? { rendererConfig: o.rendererConfig } : {}),
      ...(o.extraRenderers ? { extraRenderers: o.extraRenderers } : {}),
    });
    // Compose: scene contains the force-graph; ambient + directional
    // lights give the spheres/cylinders shape.
    (renderObjs.objects as (objs: unknown[]) => unknown)([forceGraph]);
    (renderObjs.lights as (lights: unknown[]) => unknown)([
      new ThreeBundle.AmbientLight(0xcccccc, Math.PI),
      new ThreeBundle.DirectionalLight(0xffffff, 0.6 * Math.PI),
    ]);
    return {
      forceGraph: forceGraph as State["forceGraph"],
      renderObjs: renderObjs as unknown as State["renderObjs"],
    };
  },

  init(domNode: HTMLElement, state: unknown) {
    const s = asState(state);

    // Wipe and stand up a positioned container so children can
    // absolute-anchor against it.
    domNode.innerHTML = "";
    s.container = document.createElement("div");
    s.container.style.position = "relative";
    domNode.appendChild(s.container);

    // Attach renderObjs to its own child div so we can layer the
    // info banner on top.
    const roDom = document.createElement("div");
    s.container.appendChild(roDom);
    (s.renderObjs as unknown as (host: HTMLElement) => unknown)(roDom);

    const camera = (s.renderObjs.camera as () => { position: { x: number; y: number; z: number }; lookAt: (v: { x: number; y: number; z: number }) => void })();
    const renderer = (s.renderObjs.renderer as () => {
      domElement: HTMLElement;
      useLegacyLights?: boolean;
    })();
    const controls = (s.renderObjs.controls as () => {
      enabled?: boolean;
      domElement?: HTMLElement;
      _status?: unknown;
      _onPointerCancel?: () => void;
    } | null)();
    if (controls) controls.enabled = !!s.enableNavigationControls;
    s.lastSetCameraZ = camera.position.z;

    // Loading indicator — populated by three-forcegraph's
    // onLoading/onFinishLoading hooks below.
    const infoElem = document.createElement("div");
    infoElem.className = "graph-info-msg";
    infoElem.textContent = "";
    s.container.appendChild(infoElem);

    // ── force-graph wiring ────────────────────────────────────
    const fg = s.forceGraph as unknown as {
      onLoading: (cb: () => void) => typeof fg;
      onFinishLoading: (cb: () => void) => typeof fg;
      onUpdate: (cb: () => void) => typeof fg;
      onFinishUpdate: (cb: () => void) => typeof fg;
      graphData: () => State["graphData"];
      position?: { x: number; y: number; z: number };
    };
    fg.onLoading(() => {
      infoElem.textContent = "Loading...";
    });
    fg.onFinishLoading(() => {
      infoElem.textContent = "";
    });
    fg.onUpdate(() => {
      s.graphData = fg.graphData();
      // Auto-frame the camera on the initial layout, but only if
      // the user hasn't manually moved it.
      if (
        camera.position.x === 0 &&
        camera.position.y === 0 &&
        camera.position.z === s.lastSetCameraZ &&
        s.graphData.nodes.length
      ) {
        camera.lookAt(fg.position ?? { x: 0, y: 0, z: 0 });
        const targetZ =
          Math.cbrt(s.graphData.nodes.length) * CAMERA_DISTANCE2NODES_FACTOR;
        camera.position.z = targetZ;
        s.lastSetCameraZ = targetZ;
      }
    });

    fg.onFinishUpdate(() => {
      // Drag-controls lifecycle: dispose the previous instance if
      // there is one (deferred when a drag is in flight), then
      // build a fresh one over the current node mesh list.
      if (s._dragControls) {
        const dragging = s.graphData.nodes.find(
          (node) => node.__initialFixedPos && !node.__disposeControlsAfterDrag,
        );
        if (dragging) {
          dragging.__disposeControlsAfterDrag = true;
        } else {
          (s._dragControls as ThreeDragControls).dispose();
        }
        s._dragControls = undefined;
      }

      if (
        s.enableNodeDrag &&
        s.enablePointerInteraction &&
        s.forceEngine === "d3"
      ) {
        const nodeObjs = s.graphData.nodes
          .map((n) => (n as { __threeObj?: Object3D }).__threeObj)
          .filter((o): o is Object3D => !!o);
        const dragControls = new ThreeDragControls(
          nodeObjs,
          camera as unknown as ConstructorParameters<typeof ThreeDragControls>[1],
          renderer.domElement,
        );
        s._dragControls = dragControls;

        dragControls.addEventListener("dragstart", (event: unknown) => {
          const ev = event as {
            object: Object3D & {
              __initialPos?: { clone: () => unknown };
              __prevPos?: { clone: () => unknown };
              position: {
                clone: () => unknown;
              };
            };
          };
          const nodeObj = getGraphObj(ev.object);
          if (!nodeObj) return;
          if (controls) controls.enabled = false;
          ev.object.__initialPos = ev.object.position.clone() as never;
          ev.object.__prevPos = ev.object.position.clone() as never;
          const node = nodeObj.__data as Record<string, unknown>;
          if (!node.__initialFixedPos) {
            node.__initialFixedPos = {
              fx: node.fx,
              fy: node.fy,
              fz: node.fz,
            };
          }
          if (!node.__initialPos) {
            node.__initialPos = { x: node.x, y: node.y, z: node.z };
          }
          // Lock node at its current position for the duration of
          // the drag.
          for (const c of ["x", "y", "z"] as const) {
            (node as Record<string, unknown>)[`f${c}`] = node[c];
          }
          renderer.domElement.classList.add("grabbable");
        });

        // Drag-commit threshold (world units). Drag events whose
        // cumulative motion from the press position stays inside this
        // bubble are treated as click-jitter and skipped — they don't
        // pin fx/fy/fz, don't reheat the simulation, don't fire
        // onNodeDrag, don't flip __dragged. Once a single drag event
        // crosses the threshold we set __dragCommitted = true on the
        // node and every subsequent event commits normally for the
        // rest of the gesture, so a slow start doesn't make the rest
        // of the drag feel laggy.
        //
        // Picked empirically: a HiDPI trackpad click commonly emits
        // 2–6 px of jitter, which at default camera distance maps to
        // ~0.5–1.5 world units. Two units leaves comfortable headroom
        // without making intentional small drags feel sticky.
        const DRAG_COMMIT_THRESHOLD_SQ = 4; // (2 world units)²
        dragControls.addEventListener("drag", (event: unknown) => {
          const ev = event as {
            object: Object3D & {
              __graphObjType?: string;
              __initialPos?: { clone: () => unknown };
              __prevPos?: { copy: (p: unknown) => unknown };
              position: {
                clone: () => { sub: (p: unknown) => { x: number; y: number; z: number } };
                x: number;
                y: number;
                z: number;
                copy: (p: unknown) => unknown;
              };
            };
          };
          const nodeObj = getGraphObj(ev.object);
          if (!nodeObj) return;

          if (!Object.prototype.hasOwnProperty.call(ev.object, "__graphObjType")) {
            // Dragging a child of the node — re-route the motion
            // delta to the node group itself and reset the child to
            // its starting position so the visual drag follows the
            // group rather than the child geometry.
            const initPos = ev.object.__initialPos!;
            const prevPos = ev.object.__prevPos!;
            const newPos = ev.object.position;
            const delta = (newPos.clone() as { sub: (p: unknown) => unknown }).sub(
              prevPos as unknown,
            );
            (nodeObj.position as unknown as {
              add: (d: unknown) => unknown;
            }).add(delta);
            (prevPos as { copy: (p: unknown) => unknown }).copy(newPos as unknown);
            (newPos as { copy: (p: unknown) => unknown }).copy(initPos as unknown);
          }

          const node = nodeObj.__data as Record<string, number | undefined> & {
            __dragCommitted?: boolean;
          };
          const newPos = nodeObj.position as unknown as {
            x: number;
            y: number;
            z: number;
          };
          // Click-jitter guard. Compare against the press-time origin
          // captured in `__initialPos` (set by the dragstart handler).
          // We only check until the gesture commits — once past the
          // threshold the rest of the drag passes through.
          if (!node.__dragCommitted) {
            const init = node.__initialPos as
              | { x: number; y: number; z: number }
              | undefined;
            const ix = init?.x ?? newPos.x;
            const iy = init?.y ?? newPos.y;
            const iz = init?.z ?? newPos.z;
            const dx = newPos.x - ix;
            const dy = newPos.y - iy;
            const dz = newPos.z - iz;
            if (dx * dx + dy * dy + dz * dz < DRAG_COMMIT_THRESHOLD_SQ) {
              // Pretend nothing happened — leave fx/fy/fz pinned to
              // node.x/y/z (set by dragstart) so the node visually
              // stays put while the user is making up their mind.
              return;
            }
            node.__dragCommitted = true;
          }
          const translate = {
            x: newPos.x - (node.x ?? 0),
            y: newPos.y - (node.y ?? 0),
            z: newPos.z - (node.z ?? 0),
          };
          for (const c of ["x", "y", "z"] as const) {
            (node as Record<string, number>)[`f${c}`] = newPos[c];
            (node as Record<string, number>)[c] = newPos[c];
          }
          (s.forceGraph as unknown as {
            d3AlphaTarget: (a: number) => { resetCountdown: () => unknown };
          })
            .d3AlphaTarget(0.3)
            .resetCountdown();
          (node as Record<string, unknown>).__dragged = true;
          s.onNodeDrag?.(node, translate);
        });

        dragControls.addEventListener("dragend", (event: unknown) => {
          const ev = event as {
            object: Object3D & {
              __initialPos?: unknown;
              __prevPos?: unknown;
            };
          };
          const nodeObj = getGraphObj(ev.object);
          if (!nodeObj) return;
          delete ev.object.__initialPos;
          delete ev.object.__prevPos;

          const node = nodeObj.__data as Record<string, unknown>;
          if (node.__disposeControlsAfterDrag) {
            dragControls.dispose();
            delete node.__disposeControlsAfterDrag;
          }
          const initFixedPos = node.__initialFixedPos as
            | Record<string, unknown>
            | undefined;
          const initPos = node.__initialPos as
            | { x: number; y: number; z: number }
            | undefined;
          const translate = initPos
            ? {
                x: initPos.x - (node.x as number),
                y: initPos.y - (node.y as number),
                z: initPos.z - (node.z as number),
              }
            : { x: 0, y: 0, z: 0 };
          if (initFixedPos) {
            for (const c of ["x", "y", "z"] as const) {
              const fc = `f${c}`;
              if (initFixedPos[fc] === undefined) delete node[fc];
            }
            delete node.__initialFixedPos;
            delete node.__initialPos;
            // Reset the drag-commit gate so the next press starts in
            // the "click-jitter ignored" zone again.
            delete node.__dragCommitted;
            if (node.__dragged) {
              delete node.__dragged;
              s.onNodeDragEnd?.(node, translate);
            }
          }

          (s.forceGraph as unknown as {
            d3AlphaTarget: (a: number) => { resetCountdown: () => unknown };
          })
            .d3AlphaTarget(0)
            .resetCountdown();

          if (s.enableNavigationControls && controls) {
            controls.enabled = true;
            // Cancel any pressed status the fly controls may have
            // latched onto, then synthesise a pointerup so the
            // controls don't try to keep dragging the camera.
            controls._status && controls._onPointerCancel?.();
            controls.domElement?.ownerDocument?.dispatchEvent(
              new PointerEvent("pointerup", { pointerType: "touch" }),
            );
          }
          renderer.domElement.classList.remove("grabbable");
        });
      }
    });

    // ── renderObjs wiring ─────────────────────────────────────
    // `three.REVISION` is a string literal at runtime; coerce
    // before comparing.
    if (parseInt(REVISION as unknown as string, 10) < 155) {
      // Old default behaviour on three < 155 so colour-space
      // conversion doesn't double-apply.
      renderer.useLegacyLights = false;
    }
    const ro = s.renderObjs as unknown as {
      hoverOrderComparator: (cmp: (a: Object3D, b: Object3D) => number) => typeof ro;
      tooltipContent: (fn: (o: Object3D) => string) => typeof ro;
      hoverDuringDrag: (b: boolean) => typeof ro;
      onHover: (cb: (o: Object3D | null) => void) => typeof ro;
      clickAfterDrag: (b: boolean) => typeof ro;
      onClick: (cb: (o: Object3D | null, ev: MouseEvent) => void) => typeof ro;
      onRightClick: (cb: (o: Object3D | null, ev: MouseEvent) => void) => typeof ro;
    };
    ro.hoverOrderComparator((a, b) => {
      // Prefer graph objects; among them prefer nodes over links so
      // a hover on a node-with-overlapping-link picks the node.
      const aObj = getGraphObj(a);
      if (!aObj) return 1;
      const bObj = getGraphObj(b);
      if (!bObj) return -1;
      const isNode = (o: GraphObj): number =>
        o.__graphObjType === "node" ? 1 : 0;
      return isNode(bObj) - isNode(aObj);
    })
      .tooltipContent((obj) => {
        const graphObj = getGraphObj(obj);
        if (!graphObj) return "";
        const labelAccessor = s[`${graphObj.__graphObjType}Label`];
        return (accessorFn(labelAccessor)(graphObj.__data) as string) || "";
      })
      .hoverDuringDrag(false)
      .onHover((obj) => {
        const hoverObj = getGraphObj(obj);
        if (hoverObj === s.hoverObj) return;
        const prevObj = s.hoverObj ?? null;
        const prevObjType = prevObj ? prevObj.__graphObjType : null;
        const prevObjData = prevObj ? prevObj.__data : null;
        const objType = hoverObj ? hoverObj.__graphObjType : null;
        const objData = hoverObj ? hoverObj.__data : null;
        // Fire `onXHover(null, prev)` only when the kind actually
        // changed — i.e. transition between Node → Link or X → none.
        if (prevObjType && prevObjType !== objType) {
          const fn = s[
            `on${prevObjType === "node" ? "Node" : "Link"}Hover`
          ] as ((cur: unknown, prev: unknown) => void) | undefined;
          fn?.(null, prevObjData);
        }
        if (objType) {
          const fn = s[
            `on${objType === "node" ? "Node" : "Link"}Hover`
          ] as ((cur: unknown, prev: unknown) => void) | undefined;
          fn?.(objData, prevObjType === objType ? prevObjData : null);
        }
        const clickFn = hoverObj
          ? s[`on${objType === "node" ? "Node" : "Link"}Click`]
          : s.onBackgroundClick;
        const shouldShowPointer =
          !!clickFn &&
          (accessorFn(s.showPointerCursor)(objData) as boolean);
        renderer.domElement.classList[shouldShowPointer ? "add" : "remove"](
          "clickable",
        );
        s.hoverObj = hoverObj;
      })
      .clickAfterDrag(false)
      .onClick((obj, ev) => {
        const graphObj = getGraphObj(obj);
        if (graphObj) {
          const fn = s[
            `on${graphObj.__graphObjType === "node" ? "Node" : "Link"}Click`
          ] as ((d: unknown, ev: MouseEvent) => void) | undefined;
          fn?.(graphObj.__data, ev);
        } else if (s.onBackgroundClick) {
          (s.onBackgroundClick as (ev: MouseEvent) => void)(ev);
        }
      })
      .onRightClick((obj, ev) => {
        const graphObj = getGraphObj(obj);
        if (graphObj) {
          const fn = s[
            `on${graphObj.__graphObjType === "node" ? "Node" : "Link"}RightClick`
          ] as ((d: unknown, ev: MouseEvent) => void) | undefined;
          fn?.(graphObj.__data, ev);
        } else if (s.onBackgroundRightClick) {
          (s.onBackgroundRightClick as (ev: MouseEvent) => void)(ev);
        }
      });

    // Kick off the rAF loop.
    (this as unknown as { _animationCycle: () => void })._animationCycle();
  },

  update() {
    // No-op — all reactive work lives in per-prop onChange handlers.
  },
});

export default ForceGraph3D;
