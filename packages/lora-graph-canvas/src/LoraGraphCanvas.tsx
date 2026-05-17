import {
  forwardRef,
  useCallback,
  useEffect,
  useMemo,
  useRef,
  useState,
  type CSSProperties,
} from "react";
import type {
  GraphMode,
  LinkObject,
  LoraGraphCanvasHandle,
  LoraGraphCanvasProps,
  NodeObject,
  ToolId,
} from "./types";
import { useGraphData } from "./hooks/useGraphData";
import { useGraphEngine } from "./hooks/useGraphEngine";
import { useResizeObserver } from "./hooks/useResizeObserver";
import { useGraphSelection } from "./hooks/useGraphSelection";
import { useShiftHeld } from "./hooks/useShiftHeld";
import { useAutoIndexNeighbors } from "./hooks/useAutoIndexNeighbors";
import { useMarqueeAndCursor } from "./hooks/useMarqueeAndCursor";
import { useClickToleranceShim } from "./hooks/useClickToleranceShim";
import { useGraphClipboard } from "./hooks/useGraphClipboard";
import { useGraphDeleteGate } from "./hooks/useGraphDeleteGate";
import { useGraphKeybindings } from "./hooks/useGraphKeybindings";
import { useGraphForces } from "./hooks/useGraphForces";
import { useAccessorOverrides } from "./hooks/useAccessorOverrides";
import { useHoverState } from "./hooks/useHoverState";
import { useLabelRenderer } from "./hooks/useLabelRenderer";
import { useLinkLabelRenderer } from "./hooks/useLinkLabelRenderer";
import { usePerfTierDefaults } from "./hooks/usePerfTierDefaults";
import { useImperativeGraphHandle } from "./hooks/useImperativeGraphHandle";
import { usePrefersReducedMotion } from "./hooks/usePrefersReducedMotion";
import { GraphToolbar } from "./tools/GraphToolbar";
import { ContextMenu, type ContextMenuItem } from "./tools/ContextMenu";
import { HoverTooltip } from "./tools/HoverTooltip";
import { MarqueeOverlay } from "./tools/MarqueeOverlay";
import { SelectionPanel } from "./tools/SelectionPanel";
import { GroupLegend } from "./tools/GroupLegend";
import { ModeToggle } from "./tools/ModeToggle";
import { OptionsMenu, type OptionItem } from "./tools/OptionsMenu";
import { drawBackgroundGrid } from "./utils/grid";
import { SNAP_IN } from "./utils/geometry";
import { themeToStyle } from "./utils/themeStyle";
import { downloadBlob, downloadScreenshot } from "./utils/download";
import "./theme/styles.css";

function LoraGraphCanvasInner<
  N extends NodeObject = NodeObject,
  L extends LinkObject = LinkObject,
>(
  props: LoraGraphCanvasProps<N, L>,
  ref: React.Ref<LoraGraphCanvasHandle<N, L>>,
) {
  const {
    data: controlledData,
    defaultData,
    onDataChange,
    mode: controlledMode,
    defaultMode,
    onModeChange,
    width: widthProp,
    height: heightProp,
    className,
    style,
    theme,
    tools = true,
    selection: selectionMode = "multi",
    showContextMenu = true,
    showLegend = false,
    showGrid = false,
    showLabels = false,
    enableClipboard = true,
    // Tooltips are the most common reason hosts wire `nodeLabel`, so
    // having them disabled by default made the prop silently invisible
    // on a working host config. Hosts that want chrome-free canvas
    // (e.g. an embedded thumbnail) can still opt out.
    enableTooltip = true,
    introZoom = true,
    focusOnClick = false,
    highlightNeighborsOnHover = true,
    // Highlight has nothing to walk without `_neighbors`/`_links` on
    // each node, so default this to whatever the highlight is —
    // flipping highlight on without the index would silently no-op.
    // Host can still override either knob independently.
    autoIndexNeighbors = highlightNeighborsOnHover,
    collideNodes = false,
    fixOnDrop = true,
    beeswarm = false,
    onSelectionChange,
    onNodeClick,
    onNodeRightClick,
    onLinkClick,
    onLinkRightClick,
    onBackgroundClick,
    onBackgroundRightClick,
    onNodeDrag,
    onNodeDragEnd,
    onNodeHover,
    onLinkHover,
    onNodeDoubleClick,
    onCopy,
    onCut,
    onPaste,
    onBeforeNodeDelete,
    onBeforeLinkDelete,
    onNodeDeleted,
    onLinkDeleted,
    onRenderFramePre,
    nodeColor,
    nodeLabel,
    nodeVal,
    nodeAutoColorBy,
    nodeRelSize,
    nodeVisibility,
    nodeCanvasObject,
    linkColor,
    linkLabel,
    linkWidth,
    linkHoverPrecision,
    backgroundColor,
    enableNavigationControls,
    performanceProfile,
    dagMode: dagModeProp,
  } = props;

  // ─── Mode (controlled / uncontrolled) ────────────────────────────
  const [internalMode, setInternalMode] = useState<GraphMode>(
    controlledMode ?? defaultMode ?? "2d",
  );
  const isModeControlled = controlledMode !== undefined;
  const mode = isModeControlled ? controlledMode : internalMode;
  // Trampoline for the "about to switch mode" side effects. The 2D ↔
  // 3D mode switch triggers a full kapsule remount; a few pieces of
  // React-side state — in-flight gestures whose coords are now stale,
  // the saved-view focus restore, the open context menu — need to be
  // cleared before the new engine takes over. (Engine-owned state
  // like the camera survives the remount via useGraphEngine itself.)
  // The body is wired up further down once the engine is in scope;
  // setMode reads through this ref so it can stay early in the file
  // (its identity has to be stable for every downstream useCallback
  // that lists it as a dep).
  const beforeModeChangeRef = useRef<(() => void) | null>(null);
  const setMode = useCallback(
    (next: GraphMode) => {
      if (next === mode) return;
      beforeModeChangeRef.current?.();
      if (!isModeControlled) setInternalMode(next);
      onModeChange?.(next);
    },
    [mode, isModeControlled, onModeChange],
  );

  // ─── Data ────────────────────────────────────────────────────────
  const dataApi = useGraphData<N, L>({
    ...(controlledData !== undefined ? { controlled: controlledData } : {}),
    ...(defaultData !== undefined ? { defaultData } : {}),
    ...(onDataChange ? { onChange: onDataChange } : {}),
  });

  useAutoIndexNeighbors(autoIndexNeighbors, dataApi.data);

  // ─── Selection ───────────────────────────────────────────────────
  const selection = useGraphSelection({
    mode: selectionMode,
    ...(onSelectionChange ? { onChange: onSelectionChange } : {}),
  });

  // ─── Active tool / engine paused state ──────────────────────────
  const [activeTool, setActiveTool] = useState<ToolId>("select");
  const [paused, setPaused] = useState(false);

  // ─── Hover-highlight state ──────────────────────────────────────
  // All the live-vs-debounced hover plumbing (label-stickiness grace
  // timer, neighbour-highlight sets, tooltip content, and the live
  // refs the marquee guard reads) lives in `useHoverState`. The host
  // sees a flat result so the canvas body stays focused on tool /
  // selection wiring.
  const hover = useHoverState<N, L>({
    highlightNeighborsOnHover,
    ...(nodeLabel !== undefined ? { nodeLabel } : {}),
    ...(linkLabel !== undefined ? { linkLabel } : {}),
    ...(onNodeHover ? { onNodeHover } : {}),
    ...(onLinkHover ? { onLinkHover } : {}),
  });
  const {
    hoverNodeId,
    hoverLinkId,
    liveHoverNodeIdRef,
    highlightedNodeIds,
    highlightedLinkIds,
    hoveredNodeSet,
    hoveredLinkSet,
    tooltipContent,
    handleNodeHover,
    handleLinkHover,
    pinHover,
  } = hover;

  // Group-legend hidden state. O(1) lookups in the wrapped accessor.
  const [hiddenGroups, setHiddenGroups] = useState<Set<string>>(
    () => new Set(),
  );

  // Options-menu-driven state. The corresponding props seed the
  // initial value; afterwards the menu's checkbox / select owns it.
  const [internalFocusOnClick, setInternalFocusOnClick] = useState(
    Boolean(focusOnClick),
  );
  const [internalShowLabels, setInternalShowLabels] = useState(
    Boolean(showLabels),
  );
  const [internalDagMode, setInternalDagMode] = useState<
    "td" | "bu" | "lr" | "rl" | "radialout" | "radialin" | null
  >(dagModeProp ?? null);
  useEffect(() => {
    setInternalFocusOnClick(Boolean(focusOnClick));
  }, [focusOnClick]);
  useEffect(() => {
    setInternalShowLabels(Boolean(showLabels));
  }, [showLabels]);
  useEffect(() => {
    setInternalDagMode(dagModeProp ?? null);
  }, [dagModeProp]);

  // Remembered view, so `focusOnClick` can restore on the second click.
  const savedViewRef = useRef<import("./engines/types").CameraState | null>(
    null,
  );
  const [focusedNodeId, setFocusedNodeId] = useState<
    string | number | null
  >(null);
  const [focusedLinkId, setFocusedLinkId] = useState<
    string | number | null
  >(null);

  // ─── Add-link in-progress source (clicked first node) ───────────
  const [linkSourceId, setLinkSourceId] = useState<string | number | null>(
    null,
  );

  // ─── Link selection — parallel to node selection so the toolbar's
  // selection-aware tools (delete, etc) can operate on either kind. ─
  const [selectedLinkIds, setSelectedLinkIds] = useState<
    Array<string | number>
  >([]);

  // ─── Context menu state ─────────────────────────────────────────
  const [menu, setMenu] = useState<{
    x: number;
    y: number;
    items: Array<ContextMenuItem | { separator: true }>;
  } | null>(null);

  const shiftHeld = useShiftHeld();
  // OS-level "reduce motion" preference. Tween durations passed through
  // this flag collapse to 0 so users on the setting see the final
  // frame immediately instead of the camera flying around.
  const reduceMotion = usePrefersReducedMotion();

  // ─── Host / engine mount ────────────────────────────────────────
  const hostRef = useRef<HTMLDivElement | null>(null);
  // Engine mount goes through a state-backed callback ref so attaching
  // the element triggers a re-render — without it, useGraphEngine would
  // never see `mountRef.current` flip from null to the div, since refs
  // don't re-render.
  const mountRef = useRef<HTMLDivElement | null>(null);
  const [mountEl, setMountEl] = useState<HTMLDivElement | null>(null);
  const handleMountRef = useCallback((el: HTMLDivElement | null) => {
    mountRef.current = el;
    setMountEl(el);
  }, []);
  const observed = useResizeObserver(hostRef);
  const width = widthProp ?? observed?.width ?? 600;
  const height = heightProp ?? observed?.height ?? 400;

  // The engine ref lets handlers below (and the marquee + clipboard
  // hooks) reach the latest engine after a 2D↔3D remount.
  const engineRef = useRef<ReturnType<typeof useGraphEngine<N, L>>>(null);

  // ─── Marquee + cursor tracking ──────────────────────────────────
  const { marquee, lastCursorRef } = useMarqueeAndCursor<N, L>({
    mount: mountEl,
    engineRef,
    selection,
    nodes: dataApi.data.nodes,
    links: dataApi.data.links,
    setSelectedLinkIds,
    liveHoverNodeIdRef,
  });
  useClickToleranceShim(mountEl);

  // ─── Centralised delete gate ────────────────────────────────────
  // Every site that removes a node or link funnels through this so the
  // host's `onBeforeNodeDelete` / `onBeforeLinkDelete` guards run with
  // consistent semantics (batched, async, post-delete callbacks).
  const deleteGate = useGraphDeleteGate<N, L>({
    dataApi,
    ...(onBeforeNodeDelete ? { beforeNode: onBeforeNodeDelete } : {}),
    ...(onBeforeLinkDelete ? { beforeLink: onBeforeLinkDelete } : {}),
    ...(onNodeDeleted ? { onNodeDeleted } : {}),
    ...(onLinkDeleted ? { onLinkDeleted } : {}),
    afterNodeDelete: () => selection.clear(),
    afterLinkDelete: () => setSelectedLinkIds([]),
  });

  // ─── Clipboard / duplicate / connect / pin primitives ───────────
  const clipboard = useGraphClipboard<N, L>({
    enableClipboard,
    dataApi,
    deleteGate,
    selection,
    setSelectedLinkIds,
    engineRef,
    lastCursorRef,
    ...(onCopy ? { onCopy } : {}),
    ...(onCut ? { onCut } : {}),
    ...(onPaste ? { onPaste } : {}),
  });

  // ─── Engine event interception ───────────────────────────────────
  // The active tool changes how clicks are interpreted. We forward the
  // host's own handlers first (they always fire), then apply the
  // tool-specific behaviour. Double-click is synthesised from the click
  // stream since the kapsule doesn't expose one.
  const lastNodeClickRef = useRef<{
    id: string | number;
    at: number;
  } | null>(null);

  const handleNodeClick = useCallback(
    (node: N, event: MouseEvent) => {
      onNodeClick?.(node, event);

      const now = performance.now();
      const last = lastNodeClickRef.current;
      const isDoubleClick =
        last && last.id === node.id && now - last.at < 280;
      lastNodeClickRef.current = { id: node.id, at: now };
      if (isDoubleClick) {
        onNodeDoubleClick?.(node, event);
        return;
      }

      if (activeTool === "add-link" && node.id !== undefined) {
        if (linkSourceId === null) {
          setLinkSourceId(node.id);
        } else if (linkSourceId !== node.id) {
          dataApi.addLink({
            source: linkSourceId,
            target: node.id,
          } as Parameters<typeof dataApi.addLink>[0]);
          setLinkSourceId(null);
        } else {
          setLinkSourceId(null);
        }
        return;
      }
      if (selectionMode !== "none" && node.id !== undefined) {
        const additive =
          event.shiftKey || event.ctrlKey || event.metaKey;
        selection.toggle(node.id, { additive });
        // Shift/ctrl/meta = mixed selection: keep any link selection
        // intact. Plain click resets it so the user has a single,
        // unambiguous selection.
        if (!additive) setSelectedLinkIds([]);
      }

      // Click-to-focus: snapshot the current camera, animate toward the
      // node along the user's current viewing angle. Re-clicking the
      // same node restores the saved camera state.
      if (
        internalFocusOnClick &&
        node.id !== undefined &&
        engineRef.current
      ) {
        const eng = engineRef.current;
        if (focusedNodeId === node.id) {
          if (savedViewRef.current) {
            eng.setCameraState(savedViewRef.current, 800);
          }
          setFocusedNodeId(null);
          savedViewRef.current = null;
        } else {
          // Only snapshot the current view when we're transitioning
          // *into* a focus state from a free camera — otherwise we'd
          // overwrite the original-view snapshot every time the user
          // jumps from one focused entity to another, losing the
          // "restore" anchor.
          if (focusedNodeId === null && focusedLinkId === null) {
            savedViewRef.current = eng.getCameraState();
          }
          eng.focusOn(
            { x: node.x ?? 0, y: node.y ?? 0, z: node.z ?? 0 },
            { distance: 120, zoom: 8, durationMs: reduceMotion ? 0 : 1000 },
          );
          setFocusedNodeId(node.id);
          setFocusedLinkId(null);
        }
      }
    },
    [
      onNodeClick,
      onNodeDoubleClick,
      selection,
      selectionMode,
      activeTool,
      linkSourceId,
      dataApi,
      internalFocusOnClick,
      focusedNodeId,
      focusedLinkId,
    ],
  );

  const handleBackgroundClick = useCallback(
    (event: MouseEvent) => {
      onBackgroundClick?.(event);
      engineRef.current?.stopAnimation();
      if (activeTool === "add-node") {
        const rect = mountRef.current?.getBoundingClientRect();
        const x = rect ? event.clientX - rect.left : event.clientX;
        const y = rect ? event.clientY - rect.top : event.clientY;
        const coords = engineRef.current?.screen2Graph(x, y) ?? { x, y };
        dataApi.addNode(undefined, {
          at: {
            x: coords.x,
            y: coords.y,
            ...(coords.z !== undefined ? { z: coords.z } : {}),
          },
        });
        return;
      }
      if (activeTool === "add-link") {
        setLinkSourceId(null);
      }
      // Shift/ctrl/meta = additive intent — preserve the running
      // selection so a stray click-through during a multi-select
      // gesture doesn't blow it away. Plain click still clears.
      const additive =
        event.shiftKey || event.ctrlKey || event.metaKey;
      if (!additive) {
        selection.clear();
        setSelectedLinkIds([]);
      }
      setMenu(null);
    },
    [onBackgroundClick, activeTool, dataApi, selection],
  );

  const handleNodeRightClick = useCallback(
    (node: N, event: MouseEvent) => {
      onNodeRightClick?.(node, event);
      if (!showContextMenu) return;
      const rect = hostRef.current?.getBoundingClientRect();
      const x = rect ? event.clientX - rect.left : event.clientX;
      const y = rect ? event.clientY - rect.top : event.clientY;
      const id = node.id;
      // When the right-clicked node belongs to a multi-selection,
      // swap in batch versions of the destructive actions so the menu
      // matches what the user can see is selected. We treat the click
      // as targeting the whole group rather than just `id`, which is
      // what users invariably want (right-click → "Delete" with 20
      // nodes highlighted should delete all 20, not silently one).
      const selectedIds = selection.selected;
      const isBatch =
        selectedIds.length > 1 && selectedIds.includes(id);
      const items: Array<ContextMenuItem | { separator: true }> = isBatch
        ? [
            {
              id: "pin-all",
              label: `Pin ${selectedIds.length} nodes`,
              onSelect: () => {
                for (const sid of selectedIds) {
                  const n = dataApi.data.nodes.find((nn) => nn.id === sid);
                  if (!n) continue;
                  dataApi.updateNode(
                    sid,
                    { fx: n.x, fy: n.y, fz: n.z } as Partial<N>,
                  );
                }
              },
            },
            {
              id: "unpin-all",
              label: `Unpin ${selectedIds.length} nodes`,
              onSelect: () => {
                for (const sid of selectedIds) {
                  dataApi.updateNode(
                    sid,
                    { fx: undefined, fy: undefined, fz: undefined } as Partial<N>,
                  );
                }
              },
            },
            { separator: true } as { separator: true },
            {
              id: "duplicate",
              label: `Duplicate ${selectedIds.length} nodes`,
              shortcut: "⌘D",
              onSelect: () => clipboard.duplicate(),
            },
            {
              id: "delete-all",
              label: `Delete ${selectedIds.length} nodes`,
              shortcut: "⌫",
              onSelect: () => {
                void deleteGate.requestNodeDelete(
                  selectedIds,
                  "contextMenu",
                );
              },
            },
          ]
        : [
            {
              id: "pin",
              label: node.fx !== undefined ? "Unpin" : "Pin",
              onSelect: () => {
                dataApi.updateNode(
                  id,
                  (node.fx !== undefined
                    ? { fx: undefined, fy: undefined, fz: undefined }
                    : { fx: node.x, fy: node.y, fz: node.z }) as Partial<N>,
                );
              },
            },
            {
              id: "connect",
              label: "Connect from here…",
              onSelect: () => {
                setActiveTool("add-link");
                setLinkSourceId(id);
              },
            },
            { separator: true } as { separator: true },
            {
              id: "delete",
              label: "Delete",
              shortcut: "⌫",
              onSelect: () => {
                void deleteGate.requestNodeDelete([id], "contextMenu");
              },
            },
          ];
      setMenu({ x, y, items });
    },
    [
      onNodeRightClick,
      showContextMenu,
      dataApi,
      selection,
      clipboard,
      deleteGate,
    ],
  );

  // Link click → select (selection logic mirrors nodes).
  const handleLinkClick = useCallback(
    (link: L, event: MouseEvent) => {
      onLinkClick?.(link, event);
      const lid = link.id;
      if (lid === undefined) return;
      const additive = event.shiftKey || event.ctrlKey || event.metaKey;
      setSelectedLinkIds((cur) => {
        const has = cur.includes(lid);
        if (!additive || selectionMode !== "multi") {
          if (has && cur.length === 1) return [];
          return [lid];
        }
        return has ? cur.filter((x) => x !== lid) : [...cur, lid];
      });
      // Shift/ctrl/meta keeps the node selection so the user can hold
      // mixed node + link selections (e.g. to delete both with one
      // keystroke). Plain click resets nodes so the click is the only
      // selection.
      if (!additive) selection.clear();

      // Click-to-focus mirrors the node path: animate the camera to
      // the link's midpoint, re-click the same link to restore. The
      // source/target are kapsule-resolved to node objects after the
      // first sim tick — guard for the pre-resolution case.
      if (internalFocusOnClick && engineRef.current) {
        const eng = engineRef.current;
        if (focusedLinkId === lid) {
          if (savedViewRef.current) {
            eng.setCameraState(savedViewRef.current, 800);
          }
          setFocusedLinkId(null);
          savedViewRef.current = null;
        } else {
          const src =
            typeof link.source === "object"
              ? (link.source as N)
              : null;
          const tgt =
            typeof link.target === "object"
              ? (link.target as N)
              : null;
          if (src && tgt && src.x !== undefined && tgt.x !== undefined) {
            if (focusedNodeId === null && focusedLinkId === null) {
              savedViewRef.current = eng.getCameraState();
            }
            const sx = src.x ?? 0;
            const sy = src.y ?? 0;
            const tx = tgt.x ?? 0;
            const ty = tgt.y ?? 0;
            const sz = src.z ?? 0;
            const tz = tgt.z ?? 0;
            const mid = {
              x: (sx + tx) / 2,
              y: (sy + ty) / 2,
              z: (sz + tz) / 2,
            };
            // Fit both endpoints in view rather than just zooming to
            // the midpoint at a fixed level — for a long link that
            // would put both nodes off-screen, and for a short link
            // it'd be needlessly zoomed out.
            //
            // 2D: pick the zoom that frames the bounding box of the
            //   two nodes within the viewport, with a 1.5× margin so
            //   the nodes themselves don't kiss the edges. Clamp so a
            //   tiny extent doesn't max the zoom out to numerical
            //   silliness.
            // 3D: the camera's FOV is ~50° (the kapsule's default), so
            //   `d = halfLen / tan(fov/2)` puts the endpoints on the
            //   frame edges; multiplied by margin to add padding.
            const margin = 1.5;
            const dx = tx - sx;
            const dy = ty - sy;
            const dz = tz - sz;
            const extentX = Math.max(Math.abs(dx), 1);
            const extentY = Math.max(Math.abs(dy), 1);
            const fitZoom = Math.min(
              width / (extentX * margin),
              height / (extentY * margin),
            );
            const zoom = Math.min(16, Math.max(0.5, fitZoom));
            const linkLength = Math.hypot(dx, dy, dz);
            // tan(25°) ≈ 0.466, derived from a 50° vertical FOV.
            const fitDistance = (linkLength / 2 / 0.466) * margin;
            const distance = Math.max(60, fitDistance);
            eng.focusOn(mid, { distance, zoom, durationMs: reduceMotion ? 0 : 1000 });
            setFocusedLinkId(lid);
            setFocusedNodeId(null);
          }
        }
      }
    },
    [
      onLinkClick,
      selection,
      selectionMode,
      internalFocusOnClick,
      focusedLinkId,
      focusedNodeId,
      width,
      height,
    ],
  );

  const handleLinkRightClick = useCallback(
    (link: L, event: MouseEvent) => {
      onLinkRightClick?.(link, event);
      if (!showContextMenu) return;
      const rect = hostRef.current?.getBoundingClientRect();
      const x = rect ? event.clientX - rect.left : event.clientX;
      const y = rect ? event.clientY - rect.top : event.clientY;
      setMenu({
        x,
        y,
        items: [
          {
            id: "delete",
            label: "Delete link",
            shortcut: "⌫",
            onSelect: () => {
              void deleteGate.requestLinkDelete(
                (l) => l === link,
                "contextMenu",
              );
            },
          },
          {
            id: "reverse",
            label: "Reverse direction",
            onSelect: () => {
              dataApi.removeLink((l) => l === link);
              dataApi.addLink({
                ...(link as object),
                source: (typeof link.target === "object"
                  ? (link.target as N).id
                  : link.target) as string | number,
                target: (typeof link.source === "object"
                  ? (link.source as N).id
                  : link.source) as string | number,
              } as Parameters<typeof dataApi.addLink>[0]);
            },
          },
        ],
      });
    },
    [onLinkRightClick, showContextMenu, dataApi, deleteGate],
  );

  // Drag-to-create-link: when the user drags a node while the add-link
  // tool is active, snap to the nearest other node within range and
  // commit the link on drag-end.
  const dragSnapTargetRef = useRef<N | null>(null);
  // Read latest nodes inside the drag callback without re-creating it
  // mid-drag (which would re-bind the kapsule's drag handler and could
  // glitch the snap preview).
  const nodesRef = useRef(dataApi.data.nodes);
  nodesRef.current = dataApi.data.nodes;

  const handleNodeDrag = useCallback(
    (node: N, translate: { x: number; y: number; z?: number }) => {
      onNodeDrag?.(node, translate);
      engineRef.current?.stopAnimation();
      // Pin the hover state to the dragged node. The kapsule clears
      // hover internally on mousedown, which after the 250 ms grace
      // timer wipes `highlightedNodeIds` + `highlightedLinkIds` and
      // makes the neighbour labels vanish mid-drag — exactly when
      // the user most wants to see what they're moving and what
      // it's connected to. Idempotent: re-pinning the same node is
      // a no-op for the ref + a redundant hover update.
      pinHover(node);

      if (activeTool === "add-link") {
        const nodes = nodesRef.current;
        const nx = node.x ?? 0;
        const ny = node.y ?? 0;
        const snapInSq = SNAP_IN * SNAP_IN;
        let nearest: N | null = null;
        let nearestDistSq = Infinity;
        for (let i = 0; i < nodes.length; i++) {
          const other = nodes[i];
          if (!other || other === node || other.id === node.id) continue;
          const ox = (other.x ?? 0) - nx;
          const oy = (other.y ?? 0) - ny;
          const dSq = ox * ox + oy * oy;
          if (dSq < nearestDistSq) {
            nearestDistSq = dSq;
            nearest = other;
          }
        }
        dragSnapTargetRef.current =
          nearest && nearestDistSq < snapInSq ? nearest : null;
      }
    },
    [onNodeDrag, activeTool, pinHover],
  );

  const handleNodeDragEnd = useCallback(
    (node: N, translate: { x: number; y: number; z?: number }) => {
      onNodeDragEnd?.(node, translate);
      // Release the drag-time hover pin. If the cursor is still over
      // the node (the common case — drag ends on the node), the
      // kapsule will re-fire `onNodeHover(node)` and the highlight
      // continues; otherwise the grace timer fades it naturally.
      pinHover(null);

      if (activeTool === "add-link") {
        const target = dragSnapTargetRef.current;
        dragSnapTargetRef.current = null;
        if (!target || target.id === undefined || node.id === undefined)
          return;
        dataApi.addLink({
          source: node.id,
          target: target.id,
        } as Parameters<typeof dataApi.addLink>[0]);
        return;
      }

      // Pin-on-drop: only the node the user actually dragged. The rest
      // of the selection stays free so the simulation can keep
      // arranging them.
      if (fixOnDrop) {
        const m = node as unknown as Record<string, unknown>;
        if (node.x !== undefined) m.fx = node.x;
        if (node.y !== undefined) m.fy = node.y;
        if (node.z !== undefined) m.fz = node.z;
      }
    },
    [onNodeDragEnd, activeTool, dataApi, fixOnDrop, pinHover],
  );

  const handleBackgroundRightClick = useCallback(
    (event: MouseEvent) => {
      onBackgroundRightClick?.(event);
      if (!showContextMenu) return;
      const rect = hostRef.current?.getBoundingClientRect();
      const x = rect ? event.clientX - rect.left : event.clientX;
      const y = rect ? event.clientY - rect.top : event.clientY;
      const mountRect = mountRef.current?.getBoundingClientRect();
      const cx = mountRect ? event.clientX - mountRect.left : event.clientX;
      const cy = mountRect ? event.clientY - mountRect.top : event.clientY;
      const coords =
        engineRef.current?.screen2Graph(cx, cy) ?? { x: cx, y: cy };
      setMenu({
        x,
        y,
        items: [
          {
            id: "add-node-here",
            label: "Add node here",
            onSelect: () => {
              dataApi.addNode(undefined, {
                at: {
                  x: coords.x,
                  y: coords.y,
                  ...(coords.z !== undefined ? { z: coords.z } : {}),
                },
              });
            },
          },
          {
            id: "fit",
            label: "Fit to view",
            shortcut: "F",
            onSelect: () => engineRef.current?.fit(400, 40),
          },
          {
            id: "toggle-mode",
            label: mode === "2d" ? "Switch to 3D" : "Switch to 2D",
            shortcut: "3",
            onSelect: () => setMode(mode === "2d" ? "3d" : "2d"),
          },
        ],
      });
    },
    [onBackgroundRightClick, showContextMenu, dataApi, mode, setMode],
  );

  // ─── Selection-aware accessors ─────────────────────────────────
  const accentColor = theme?.accent ?? "#4f8ef7";
  const selectedNodeSet = useMemo(
    () => new Set(selection.selected),
    [selection.selected],
  );
  const selectedLinkSet = useMemo(
    () => new Set(selectedLinkIds),
    [selectedLinkIds],
  );

  const accessors = useAccessorOverrides<N, L>({
    mode,
    accentColor,
    ...(nodeColor !== undefined ? { nodeColor } : {}),
    ...(linkColor !== undefined ? { linkColor } : {}),
    ...(linkWidth !== undefined ? { linkWidth } : {}),
    ...(nodeVal !== undefined ? { nodeVal } : {}),
    ...(nodeRelSize !== undefined ? { nodeRelSize } : {}),
    ...(props.nodePointerAreaPaint !== undefined
      ? { nodePointerAreaPaint: props.nodePointerAreaPaint }
      : {}),
    ...(nodeAutoColorBy !== undefined ? { nodeAutoColorBy } : {}),
    ...(nodeVisibility !== undefined ? { nodeVisibility } : {}),
    selectedNodeSet,
    selectedLinkSet,
    highlightNeighborsOnHover,
    highlightedNodeIds,
    highlightedLinkIds,
    hoverNodeId,
    hoverLinkId,
    hiddenGroups,
  });

  const nodeLabelRenderer = useLabelRenderer<N>({
    mode,
    showLabels: internalShowLabels,
    selectedNodeSet,
    hoveredNodeSet,
    accentColor,
    ...(nodeCanvasObject ? { hostNodeCanvasObject: nodeCanvasObject } : {}),
    ...(props.nodeThreeObject
      ? { hostNodeThreeObject: props.nodeThreeObject }
      : {}),
    ...(nodeLabel !== undefined ? { nodeLabel } : {}),
    ...(nodeVal !== undefined ? { nodeVal } : {}),
    ...(nodeRelSize !== undefined ? { nodeRelSize } : {}),
    ...(theme ? { theme } : {}),
  });

  const linkLabelRenderer = useLinkLabelRenderer<L>({
    mode,
    showLabels: internalShowLabels,
    selectedLinkSet,
    hoveredLinkSet,
    accentColor,
    ...(props.linkCanvasObject
      ? { hostLinkCanvasObject: props.linkCanvasObject }
      : {}),
    ...(props.linkThreeObject
      ? { hostLinkThreeObject: props.linkThreeObject }
      : {}),
    ...(props.linkPositionUpdate !== undefined
      ? { hostLinkPositionUpdate: props.linkPositionUpdate }
      : {}),
    ...(linkLabel !== undefined ? { linkLabel } : {}),
    ...(theme ? { theme } : {}),
  });

  // ─── Auto-performance tier ──────────────────────────────────────
  const perfDefaults = usePerfTierDefaults<N, L>({
    profile: performanceProfile,
    nodeCount: dataApi.data.nodes.length,
    linkCount: dataApi.data.links.length,
    mode,
  });

  // In 3D mode, suppress Three.js navigation controls while the user is
  // performing a marquee gesture (or just holding Shift in anticipation
  // of one) so the camera doesn't pan/rotate alongside the rectangle.
  const suppressNav = mode === "3d" && (shiftHeld || marquee !== null);

  // Resolve the canvas background once for both modes. The unified
  // engine renders through the 3D kapsule in BOTH presentation modes,
  // so its `#000011` navy default bleeds through unless we always
  // override. Computed up here so the memo below doesn't have to
  // re-derive it inline. Host's `backgroundColor` prop wins; otherwise
  // fall back to the theme background (skipping `"transparent"` since
  // WebGL needs a concrete clear colour) and finally white.
  const resolvedBackgroundColor = useMemo(() => {
    if (backgroundColor !== undefined) return backgroundColor;
    if (theme?.background && theme.background !== "transparent") {
      return theme.background;
    }
    return "#ffffff";
  }, [backgroundColor, theme?.background]);

  const engineProps = useMemo<LoraGraphCanvasProps<N, L>>(() => {
    // Build the prop bag imperatively so we can fill engine-canvas
    // defaults AFTER the host's `...props` spread without those
    // defaults getting clobbered by `props.<key> === undefined` (which
    // a destructured-but-unset prop still spreads through). This was
    // the source of two visual gotchas:
    //
    //   1. `backgroundColor` defaulting to the 3D kapsule's `#000011`
    //      navy in 2D presentation (engine is always the 3D kapsule),
    //      causing a dark→light flash on the first 2D→3D toggle.
    //   2. `linkColor` reverting to the kapsule's faint `#f0f0f0` at
    //      opacity 0.2 whenever no selection / hover overlay was
    //      active, in either mode.
    const out: Record<string, unknown> = {
      ...perfDefaults,
      ...props,
      // DAG orientation is owned by the options menu (seeded from
      // `props.dagMode` on mount); pass the live value through so a UI
      // change re-lays-out the graph immediately.
      dagMode: internalDagMode as Exclude<typeof internalDagMode, undefined>,
    };
    // Fill kapsule-default holes only when the host didn't supply a
    // value. Same defaults in both presentation modes — the engine is
    // the 3D kapsule throughout, so its dim defaults bleed through
    // identically in 2D and 3D and must be overridden uniformly.
    if (out.backgroundColor === undefined) {
      out.backgroundColor = resolvedBackgroundColor;
    }
    if (out.linkColor === undefined) {
      // Kept in lock-step with `LINK_DEFAULT` inside
      // useAccessorOverrides — otherwise the un-stateful link colour
      // would shift the instant any selection appears (when the
      // wrapper takes over and returns its own default).
      out.linkColor = "rgba(96, 102, 110, 0.55)";
    }
    if (out.linkOpacity === undefined) out.linkOpacity = 1;
    if (out.nodeOpacity === undefined) out.nodeOpacity = 1;
    // Selection / hover overlay wrappers — when active they win over
    // any host-supplied accessor or the canvas defaults above. When
    // inactive the memo returns the host's accessor (or undefined) so
    // we don't inject a per-frame wrapper for nothing.
    if (accessors.nodeColor !== undefined) out.nodeColor = accessors.nodeColor;
    if (accessors.linkColor !== undefined) out.linkColor = accessors.linkColor;
    if (accessors.linkWidth !== undefined) out.linkWidth = accessors.linkWidth;
    if (accessors.nodeVal !== undefined) out.nodeVal = accessors.nodeVal;
    if (accessors.nodePointerAreaPaint !== undefined) {
      out.nodePointerAreaPaint = accessors.nodePointerAreaPaint;
    }
    // On-canvas label rendering: draw text beneath each node. Mode
    // "after" so the engine's default circle is drawn first, our label
    // sits on top.
    //
    // IMPORTANT: pass the mode as a *function*, not a string. The
    // kapsule's accessor-fn helper treats string values as per-node
    // property-name lookups (`node["after"]`), so passing `"after"`
    // resolves to undefined for every node and the canvas object is
    // never invoked.
    if (nodeLabelRenderer.canvasObject) {
      out.nodeCanvasObject = nodeLabelRenderer.canvasObject;
      out.nodeCanvasObjectMode = (() => "after") as () => "after";
    }
    // Same pattern for links — draw a small label along the link when
    // a link is selected (or all links when showLabels is on).
    if (linkLabelRenderer.canvasObject) {
      out.linkCanvasObject = linkLabelRenderer.canvasObject;
      out.linkCanvasObjectMode = (() => "after") as () => "after";
    }
    // 3D node label sprite under the sphere — extend keeps the
    // default node mesh visible and adds our caption as a child.
    if (nodeLabelRenderer.threeObject) {
      out.nodeThreeObject = nodeLabelRenderer.threeObject;
      out.nodeThreeObjectExtend = true;
    }
    // 3D link label sprite at the midpoint. Position update keeps the
    // sprite glued to the live link coords each tick; extend preserves
    // the kapsule's default line/cylinder rendering.
    if (linkLabelRenderer.threeObject) {
      out.linkThreeObject = linkLabelRenderer.threeObject;
      out.linkThreeObjectExtend = true;
      if (linkLabelRenderer.positionUpdate) {
        out.linkPositionUpdate = linkLabelRenderer.positionUpdate;
      }
    }
    // Generous hit area for thin links — without this, edges are hard
    // to click. Hosts can override by passing their own value.
    out.linkHoverPrecision = linkHoverPrecision ?? 8;
    out.enableNavigationControls = suppressNav
      ? false
      : (enableNavigationControls ?? true);
    out.onNodeClick = handleNodeClick;
    out.onNodeRightClick = handleNodeRightClick;
    out.onLinkClick = handleLinkClick;
    out.onLinkRightClick = handleLinkRightClick;
    out.onBackgroundClick = handleBackgroundClick;
    out.onBackgroundRightClick = handleBackgroundRightClick;
    out.onNodeDrag = handleNodeDrag;
    out.onNodeDragEnd = handleNodeDragEnd;
    // Swallow the engine's HTML tooltip — we render our own.
    out.nodeLabel = () => "";
    out.linkLabel = () => "";
    out.onNodeHover = handleNodeHover;
    out.onLinkHover = handleLinkHover;
    // Background grid via the render-frame-pre hook. Wraps any
    // user-provided callback so we don't clobber it.
    if (showGrid) {
      out.onRenderFramePre = (
        ctx: CanvasRenderingContext2D,
        scale: number,
      ) => {
        const gridOpts = typeof showGrid === "object" ? showGrid : {};
        drawBackgroundGrid(ctx, scale, gridOpts);
        onRenderFramePre?.(ctx, scale);
      };
    }
    // Group-legend visibility: only inject the wrapped accessor when
    // either the legend filter is in use or the host has supplied
    // their own nodeVisibility. Otherwise leave the prop alone so the
    // kapsule uses its default.
    if (accessors.nodeVisibility) out.nodeVisibility = accessors.nodeVisibility;
    return out as LoraGraphCanvasProps<N, L>;
  }, [
      props,
      internalDagMode,
      resolvedBackgroundColor,
      accessors.nodeColor,
      accessors.linkColor,
      accessors.linkWidth,
      accessors.nodeVal,
      accessors.nodePointerAreaPaint,
      accessors.nodeVisibility,
      perfDefaults,
      suppressNav,
      linkHoverPrecision,
      enableNavigationControls,
      handleNodeClick,
      handleNodeRightClick,
      handleLinkClick,
      handleLinkRightClick,
      handleBackgroundClick,
      handleBackgroundRightClick,
      handleNodeDrag,
      handleNodeDragEnd,
      handleNodeHover,
      handleLinkHover,
      showGrid,
      onRenderFramePre,
      reduceMotion,
      nodeLabelRenderer.canvasObject,
      nodeLabelRenderer.threeObject,
      linkLabelRenderer.canvasObject,
      linkLabelRenderer.threeObject,
      linkLabelRenderer.positionUpdate,
    ],
  );

  const engine = useGraphEngine<N, L>({
    mount: mountEl,
    mode,
    width,
    height,
    data: dataApi.data,
    props: engineProps,
    paused,
    // 0 = snap mode transitions instantly for reduce-motion users.
    // The hook treats undefined as "use the default 800ms," so we
    // only override on the truthy branch.
    ...(reduceMotion ? { modeTransitionMs: 0 } : {}),
  });
  engineRef.current = engine;

  // Body of the trampoline declared near `setMode`. We assign rather
  // than memoise because every dependency here is a stable useState
  // setter or a ref. Engine-owned state (camera, pause) survives the
  // remount via useGraphEngine itself, so nothing engine-side to do
  // here.
  beforeModeChangeRef.current = () => {
    // The focus-restore plumbing snapshots a `CameraState` whose
    // `mode` field is mode-specific; restoring it through a kapsule
    // of the wrong mode would silently no-op (see
    // `engine.setCameraState`). Clear so the next focus interaction
    // starts fresh.
    setFocusedNodeId(null);
    setFocusedLinkId(null);
    savedViewRef.current = null;
    // In-progress add-link gesture: the first click happened in the
    // old mode. Carrying the source id into the new mode is more
    // confusing than helpful — the user can't see the pending arrow
    // anyway.
    setLinkSourceId(null);
    // Context menu coords are pinned to the host's bounding rect
    // which is still valid, but the menu items themselves
    // (e.g. "Switch to 3D / 2D") are stale relative to the new mode.
    setMenu(null);
  };

  // ─── First-load auto-fit ────────────────────────────────────────
  // Fires once per component lifetime, ~150ms after the engine is
  // ready and data is non-empty — long enough for the first few
  // simulation ticks to spread nodes out so the bbox is meaningful,
  // short enough that the user perceives the graph as "appearing
  // already framed". We don't wait on onEngineStop because the
  // simulation can take many seconds (or never, with pinned nodes)
  // to fully settle, and the kapsule's `cbrt(n)*170` heuristic on
  // first data load is disabled in createEngineUnified so this is
  // the only camera motion the user sees on first load.
  const hasAutoFitRef = useRef(false);
  const nodeCount = dataApi.data.nodes.length;
  useEffect(() => {
    if (!introZoom) return;
    if (!engine) return;
    if (hasAutoFitRef.current) return;
    if (nodeCount === 0) return;
    hasAutoFitRef.current = true;
    const tweenMs = reduceMotion ? 0 : 1000;
    const timer = setTimeout(() => {
      engineRef.current?.fit(tweenMs, 40);
    }, 150);
    return () => clearTimeout(timer);
  }, [engine, introZoom, nodeCount]);

  useGraphForces<N, L>({
    engine,
    collideNodes,
    beeswarm,
    ...(nodeRelSize !== undefined ? { nodeRelSize } : {}),
  });

  // ─── JSON import / export ───────────────────────────────────────
  const fileInputRef = useRef<HTMLInputElement | null>(null);
  const exportJSON = useCallback(
    () =>
      JSON.stringify(
        {
          nodes: dataApi.data.nodes.map((n) => {
            // Drop ephemeral simulation fields so re-imports get fresh
            // layout values. `_neighbors` / `_links` are object-graph
            // refs populated by `autoIndexNeighbors` — they form a
            // cycle that breaks JSON.stringify, so strip them too.
            const {
              // eslint-disable-next-line @typescript-eslint/no-unused-vars
              vx, vy, vz, index, _neighbors, _links,
              ...rest
            } = n as N & Record<string, unknown>;
            return rest;
          }),
          links: dataApi.data.links,
        },
        null,
        2,
      ),
    [dataApi.data],
  );
  const importJSON = useCallback(
    (json: string) => {
      const parsed = JSON.parse(json) as { nodes: N[]; links: L[] };
      if (
        !parsed ||
        !Array.isArray(parsed.nodes) ||
        !Array.isArray(parsed.links)
      ) {
        throw new Error("invalid graph JSON");
      }
      dataApi.setData(parsed);
      selection.clear();
      setSelectedLinkIds([]);
    },
    [dataApi, selection],
  );
  const downloadJSON = useCallback(
    (filename = `graph-${Date.now()}.json`) => {
      const blob = new Blob([exportJSON()], { type: "application/json" });
      downloadBlob(blob, filename);
    },
    [exportJSON],
  );

  // ─── Toolbar dispatch ────────────────────────────────────────────
  const handleToolSelect = useCallback(
    (id: ToolId) => {
      switch (id) {
        case "select":
        case "pan":
        case "add-node":
        case "add-link":
          setActiveTool(id);
          break;
        case "delete":
          void deleteGate.requestMixedDelete(
            selection.selected,
            selectedLinkIds,
            "toolbar",
          );
          break;
        case "duplicate":
          clipboard.duplicate();
          break;
        case "select-all":
          selection.set(dataApi.data.nodes.map((n) => n.id));
          setSelectedLinkIds(
            dataApi.data.links
              .map((l) => l.id)
              .filter((id): id is string | number => id !== undefined),
          );
          break;
        case "fit":
          engine?.fit(400, 40);
          break;
        case "zoom-in":
          if (engine) engine.zoom((engine.getZoom?.() ?? 1) * 1.2, 200);
          break;
        case "zoom-out":
          if (engine) engine.zoom((engine.getZoom?.() ?? 1) / 1.2, 200);
          break;
        case "pause":
          setPaused(true);
          break;
        case "resume":
          setPaused(false);
          break;
        case "screenshot":
          downloadScreenshot(engine?.getCanvasElement());
          break;
        case "export-json":
          downloadJSON();
          break;
        case "import-json":
          fileInputRef.current?.click();
          break;
        case "toggle-mode":
          setMode(mode === "2d" ? "3d" : "2d");
          break;
      }
    },
    [
      engine,
      dataApi,
      selection,
      mode,
      setMode,
      selectedLinkIds,
      clipboard,
      downloadJSON,
      deleteGate,
    ],
  );

  useGraphKeybindings<N, L>({
    engine,
    dataApi,
    deleteGate,
    selection,
    mode,
    setMode,
    selectedLinkIds,
    setSelectedLinkIds,
    setLinkSourceId,
    setActiveTool,
    enableClipboard,
    copy: clipboard.copy,
    cut: clipboard.cut,
    paste: clipboard.paste,
    duplicate: clipboard.duplicate,
    addConnectedNode: clipboard.addConnectedNode,
    togglePin: clipboard.togglePin,
    hostRef,
  });

  useImperativeGraphHandle<N, L>({
    ref,
    dataApi,
    deleteGate,
    selection,
    engine,
    mode,
    setMode,
    setPaused,
    clipboard,
    exportJSON,
    importJSON,
    downloadJSON,
  });

  // ─── Render ──────────────────────────────────────────────────────
  const hostStyle = useMemo<CSSProperties>(
    () => ({
      position: "relative",
      width: widthProp ?? "100%",
      height: heightProp ?? "100%",
      ...themeToStyle(theme),
      ...style,
    }),
    [widthProp, heightProp, theme, style],
  );

  const optionsMenuItems = useMemo<OptionItem[]>(
    () => [
      {
        id: "focus-on-click",
        kind: "toggle",
        label: "Click to focus",
        hint: "Animate the camera to a clicked node; click again to restore.",
        checked: internalFocusOnClick,
        onChange: setInternalFocusOnClick,
      },
      {
        id: "show-labels",
        kind: "toggle",
        label: "Always show labels",
        hint: "Draw every node + link label on the canvas, not just the selected ones. 2D only.",
        checked: internalShowLabels,
        onChange: setInternalShowLabels,
      },
      {
        kind: "select",
        id: "dag-mode",
        label: "DAG orientation",
        hint: "Force a hierarchical / radial layout.",
        value: internalDagMode ?? "null",
        options: [
          { value: "null", label: "off" },
          { value: "td", label: "td (top-down)" },
          { value: "bu", label: "bu (bottom-up)" },
          { value: "lr", label: "lr (left-right)" },
          { value: "rl", label: "rl (right-left)" },
          { value: "radialout" },
          { value: "radialin" },
        ],
        onChange: (next) =>
          setInternalDagMode(
            next === "null"
              ? null
              : (next as Exclude<typeof internalDagMode, null>),
          ),
      } satisfies OptionItem,
    ],
    [internalFocusOnClick, internalShowLabels, internalDagMode],
  );

  return (
    <div
      ref={hostRef}
      className={["lora-graph-canvas", className ?? ""].join(" ").trim()}
      style={hostStyle}
      data-mode={mode}
      data-tool={activeTool}
      data-paused={paused ? "true" : undefined}
      tabIndex={0}
      role="application"
      aria-label="Graph canvas"
    >
      <div ref={handleMountRef} className="lgc-engine-mount" />
      <GraphToolbar
        config={tools}
        activeTool={activeTool}
        paused={paused}
        mode={mode}
        onSelect={handleToolSelect}
      />
      {tools !== false ? (
        <>
          <OptionsMenu items={optionsMenuItems} />
          <ModeToggle
            mode={mode}
            onToggle={() => setMode(mode === "2d" ? "3d" : "2d")}
          />
        </>
      ) : null}
      {menu ? (
        <ContextMenu
          x={menu.x}
          y={menu.y}
          items={menu.items}
          onClose={() => setMenu(null)}
        />
      ) : null}
      {enableTooltip ? (
        <HoverTooltip content={tooltipContent} hostRef={hostRef} />
      ) : null}
      <MarqueeOverlay
        rect={marquee}
        {...(marquee?.count !== undefined ? { count: marquee.count } : {})}
      />
      {showLegend && nodeAutoColorBy ? (
        <GroupLegend
          nodes={dataApi.data.nodes}
          groupBy={
            nodeAutoColorBy as string | ((n: N) => string | number | null)
          }
          hidden={hiddenGroups}
          onToggle={(group) =>
            setHiddenGroups((cur) => {
              const next = new Set(cur);
              if (next.has(group)) next.delete(group);
              else next.add(group);
              return next;
            })
          }
        />
      ) : null}
      <SelectionPanel
        nodeCount={selection.selected.length}
        linkCount={selectedLinkIds.length}
        hasClipboard={clipboard.hasClipboard()}
        enableClipboard={enableClipboard}
        onDelete={() => {
          void deleteGate.requestMixedDelete(
            selection.selected,
            selectedLinkIds,
            "selectionPanel",
          );
        }}
        onDuplicate={() => clipboard.duplicate()}
        onAddConnected={() => clipboard.addConnectedNode()}
        onCopy={() => clipboard.copy()}
        onCut={() => clipboard.cut()}
        onPaste={() => clipboard.paste()}
        onClear={() => {
          selection.clear();
          setSelectedLinkIds([]);
        }}
      />
      <input
        ref={fileInputRef}
        type="file"
        accept="application/json,.json"
        style={{ display: "none" }}
        onChange={(e) => {
          const file = e.target.files?.[0];
          if (!file) return;
          file
            .text()
            .then(importJSON)
            .catch((err) => {
              console.error("[lora-graph-canvas] import failed:", err);
            });
          // Reset so picking the same file twice re-fires onChange.
          e.target.value = "";
        }}
      />
    </div>
  );
}

/** React component exporting both a default-instantiated and a generic
 *  signature. Consumers can pass `<LoraGraphCanvas<MyNode, MyLink> ... />`
 *  to constrain the node / link shape. */
export const LoraGraphCanvas = forwardRef(LoraGraphCanvasInner) as <
  N extends NodeObject = NodeObject,
  L extends LinkObject = LinkObject,
>(
  props: LoraGraphCanvasProps<N, L> & {
    ref?: React.Ref<LoraGraphCanvasHandle<N, L>>;
  },
) => ReturnType<typeof LoraGraphCanvasInner>;

(LoraGraphCanvas as unknown as { displayName: string }).displayName =
  "LoraGraphCanvas";
