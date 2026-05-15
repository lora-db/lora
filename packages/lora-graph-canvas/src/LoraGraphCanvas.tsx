import {
  forwardRef,
  useCallback,
  useEffect,
  useImperativeHandle,
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
  LoraGraphTheme,
  NodeObject,
  ToolId,
} from "./types";
import { useGraphData } from "./hooks/useGraphData";
import { useGraphEngine } from "./hooks/useGraphEngine";
import { useResizeObserver } from "./hooks/useResizeObserver";
import { useGraphSelection } from "./hooks/useGraphSelection";
import { GraphToolbar } from "./tools/GraphToolbar";
import { ContextMenu, type ContextMenuItem } from "./tools/ContextMenu";
import { HoverTooltip } from "./tools/HoverTooltip";
import { MarqueeOverlay } from "./tools/MarqueeOverlay";
import { SelectionPanel } from "./tools/SelectionPanel";
import { SNAP_IN } from "./utils/geometry";
import "./theme/styles.css";

/** Resolve an accessor against an object. Mirrors the kapsule's
 *  `accessor-fn` semantics: a function is invoked; a string is used as
 *  a property name; anything else (including undefined) is returned as
 *  is. */
function readAccessor<T, In>(
  accessor: T | string | ((obj: In) => T) | undefined,
  obj: In,
): T | undefined {
  if (typeof accessor === "function") return (accessor as (o: In) => T)(obj);
  if (typeof accessor === "string") {
    return (obj as unknown as Record<string, unknown>)[accessor] as
      | T
      | undefined;
  }
  return accessor;
}

const THEME_TO_VAR: Record<keyof LoraGraphTheme, string> = {
  background: "--lgc-bg",
  foreground: "--lgc-fg",
  border: "--lgc-border",
  accent: "--lgc-accent",
  toolbarBackground: "--lgc-toolbar-bg",
  toolbarForeground: "--lgc-toolbar-fg",
  toolbarBorder: "--lgc-toolbar-border",
  toolActiveBackground: "--lgc-tool-active-bg",
  toolHoverBackground: "--lgc-tool-hover-bg",
  tooltipBackground: "--lgc-tooltip-bg",
  tooltipForeground: "--lgc-tooltip-fg",
  menuBackground: "--lgc-menu-bg",
  menuForeground: "--lgc-menu-fg",
  menuHoverBackground: "--lgc-menu-hover-bg",
  fontFamily: "--lgc-font",
  fontSize: "--lgc-font-size",
};

function themeToStyle(theme?: Partial<LoraGraphTheme>): CSSProperties {
  if (!theme) return {};
  const out: Record<string, string> = {};
  for (const [key, value] of Object.entries(theme)) {
    if (value === undefined) continue;
    const cssVar = THEME_TO_VAR[key as keyof LoraGraphTheme];
    if (cssVar) out[cssVar] = String(value);
  }
  return out as CSSProperties;
}

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
    enableRename = true,
    enableClipboard = true,
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
    onNodeRename,
    onCopy,
    onCut,
    onPaste,
  } = props;

  // ─── Mode ────────────────────────────────────────────────────────
  const [internalMode, setInternalMode] = useState<GraphMode>(
    controlledMode ?? defaultMode ?? "2d",
  );
  const isModeControlled = controlledMode !== undefined;
  const mode = isModeControlled ? controlledMode : internalMode;
  const setMode = useCallback(
    (next: GraphMode) => {
      if (!isModeControlled) setInternalMode(next);
      onModeChange?.(next);
    },
    [isModeControlled, onModeChange],
  );

  // ─── Data ────────────────────────────────────────────────────────
  const dataApi = useGraphData<N, L>({
    ...(controlledData !== undefined ? { controlled: controlledData } : {}),
    ...(defaultData !== undefined ? { defaultData } : {}),
    ...(onDataChange ? { onChange: onDataChange } : {}),
  });

  // ─── Selection ───────────────────────────────────────────────────
  const selection = useGraphSelection({
    mode: selectionMode,
    ...(onSelectionChange ? { onChange: onSelectionChange } : {}),
  });

  // ─── Active tool / engine paused state ──────────────────────────
  const [activeTool, setActiveTool] = useState<ToolId>("select");
  const [paused, setPaused] = useState(false);

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
    items: ContextMenuItem[];
  } | null>(null);

  // ─── Hover tooltip content ──────────────────────────────────────
  const [tooltipContent, setTooltipContent] = useState<
    string | HTMLElement | null
  >(null);

  // ─── Marquee state ──────────────────────────────────────────────
  const [marquee, setMarquee] = useState<{
    x0: number;
    y0: number;
    x1: number;
    y1: number;
    additive: boolean;
  } | null>(null);

  // ─── Inline rename state ────────────────────────────────────────
  const [renaming, setRenaming] = useState<{
    id: string | number;
    x: number;
    y: number;
    value: string;
    previous: string | undefined;
  } | null>(null);

  // ─── Internal clipboard (lives for the lifetime of the
  // component instance — not the OS clipboard). ──────────────────
  const clipboardRef = useRef<Array<Partial<N>>>([]);

  // Track the last screen-space cursor over the mount so paste can
  // drop new nodes where the user expects them.
  const lastCursorRef = useRef<{ x: number; y: number } | null>(null);

  // ─── Host / engine mount ────────────────────────────────────────
  const hostRef = useRef<HTMLDivElement | null>(null);
  const mountRef = useRef<HTMLDivElement | null>(null);
  const observed = useResizeObserver(hostRef);
  const width = widthProp ?? observed?.width ?? 600;
  const height = heightProp ?? observed?.height ?? 400;

  // ─── Engine event interception ───────────────────────────────────
  // The active tool changes how clicks are interpreted. We forward the
  // host's own handlers first (they always fire), then apply the
  // tool-specific behaviour.
  // Double-click detection. The kapsule doesn't expose a
  // double-click event, so we synthesise one from the click stream:
  // two clicks on the same node within 280ms triggers a double-click.
  const lastNodeClickRef = useRef<{
    id: string | number;
    at: number;
  } | null>(null);

  // Forward ref so handleNodeClick can call beginRename even though
  // it's defined further down. (Avoids a useCallback ordering loop.)
  const beginRenameRef = useRef<((node: N) => void) | null>(null);

  const handleNodeClick = useCallback(
    (node: N, event: MouseEvent) => {
      onNodeClick?.(node, event);

      // Double-click detection. The host's onNodeDoubleClick fires
      // regardless of `enableRename`; the built-in rename only fires
      // when the feature is enabled.
      const now = performance.now();
      const last = lastNodeClickRef.current;
      const isDoubleClick =
        last && last.id === node.id && now - last.at < 280;
      lastNodeClickRef.current = { id: node.id, at: now };
      if (isDoubleClick) {
        props.onNodeDoubleClick?.(node, event);
        if (enableRename) beginRenameRef.current?.(node);
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
          // Clicked the source again — cancel.
          setLinkSourceId(null);
        }
        return;
      }
      if (selectionMode !== "none" && node.id !== undefined) {
        selection.toggle(node.id, {
          additive: event.shiftKey || event.ctrlKey || event.metaKey,
        });
        setSelectedLinkIds([]);
      }
    },
    [
      onNodeClick,
      selection,
      selectionMode,
      activeTool,
      linkSourceId,
      dataApi,
      props,
      enableRename,
    ],
  );

  // We need the engine for screen→graph projection in add-node, but
  // the engine ref lives below. The trampoline pattern via `useRef`
  // means we can read the latest engine reference inside this callback.
  const engineRef = useRef<ReturnType<typeof useGraphEngine<N, L>>>(null);

  const handleBackgroundClick = useCallback(
    (event: MouseEvent) => {
      onBackgroundClick?.(event);
      if (activeTool === "add-node") {
        const rect = mountRef.current?.getBoundingClientRect();
        const x = rect ? event.clientX - rect.left : event.clientX;
        const y = rect ? event.clientY - rect.top : event.clientY;
        const coords = engineRef.current?.screen2Graph(x, y) ?? { x, y };
        dataApi.addNode(undefined, {
          at: { x: coords.x, y: coords.y, ...(coords.z !== undefined ? { z: coords.z } : {}) },
        });
        return;
      }
      if (activeTool === "add-link") {
        setLinkSourceId(null);
      }
      selection.clear();
      setSelectedLinkIds([]);
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
      setMenu({
        x,
        y,
        items: [
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
              dataApi.removeNode(id);
              selection.clear();
            },
          },
        ] as ContextMenuItem[],
      });
    },
    [
      onNodeRightClick,
      showContextMenu,
      dataApi,
      selection,
    ],
  );

  // Link click → select (selection logic mirrors nodes). Multi-select
  // requires shift/ctrl/cmd.
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
      // Selecting a link should clear the node selection (and vice
      // versa via the existing node-click flow).
      selection.clear();
    },
    [onLinkClick, selection, selectionMode],
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
              dataApi.removeLink((l) => l === link);
              setSelectedLinkIds([]);
            },
          },
          {
            id: "reverse",
            label: "Reverse direction",
            onSelect: () => {
              // Swap source/target. We need stable refs — addLink+removeLink
              // round-trip works regardless of whether source/target are
              // ids or resolved node objects.
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
    [
      onLinkRightClick,
      showContextMenu,
      dataApi,
    ],
  );

  // ─── Hover handlers — drive the React-rendered tooltip ─────────
  const handleNodeHover = useCallback(
    (node: N | null, prev: N | null) => {
      onNodeHover?.(node, prev);
      if (!node) {
        setTooltipContent(null);
        return;
      }
      const label = readAccessor<string | HTMLElement, N>(
        props.nodeLabel,
        node,
      );
      setTooltipContent(label ?? null);
    },
    [onNodeHover, props.nodeLabel],
  );

  const handleLinkHover = useCallback(
    (link: L | null, prev: L | null) => {
      onLinkHover?.(link, prev);
      if (!link) {
        setTooltipContent(null);
        return;
      }
      const label = readAccessor<string | HTMLElement, L>(
        props.linkLabel,
        link,
      );
      setTooltipContent(label ?? null);
    },
    [onLinkHover, props.linkLabel],
  );

  // Drag-to-create-link: when the user drags a node while the add-link
  // tool is active, snap to the nearest other node within range and
  // commit the link on drag-end.
  const dragSnapTargetRef = useRef<N | null>(null);
  const handleNodeDrag = useCallback(
    (node: N, translate: { x: number; y: number; z?: number }) => {
      onNodeDrag?.(node, translate);
      if (activeTool !== "add-link") return;
      const nodes = dataApi.data.nodes;
      let nearest: N | null = null;
      let nearestDist = Infinity;
      for (const other of nodes) {
        if (other === node || other.id === node.id) continue;
        const ox = other.x ?? 0;
        const oy = other.y ?? 0;
        const nx = node.x ?? 0;
        const ny = node.y ?? 0;
        const d = Math.hypot(ox - nx, oy - ny);
        if (d < nearestDist) {
          nearestDist = d;
          nearest = other;
        }
      }
      dragSnapTargetRef.current =
        nearest && nearestDist < SNAP_IN ? nearest : null;
    },
    [onNodeDrag, activeTool, dataApi.data.nodes],
  );

  const handleNodeDragEnd = useCallback(
    (node: N, translate: { x: number; y: number; z?: number }) => {
      onNodeDragEnd?.(node, translate);
      if (activeTool !== "add-link") return;
      const target = dragSnapTargetRef.current;
      dragSnapTargetRef.current = null;
      if (!target || target.id === undefined || node.id === undefined) return;
      dataApi.addLink({
        source: node.id,
        target: target.id,
      } as Parameters<typeof dataApi.addLink>[0]);
    },
    [onNodeDragEnd, activeTool, dataApi],
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
    [
      onBackgroundRightClick,
      showContextMenu,
      dataApi,
      mode,
      setMode,
    ],
  );

  // ─── Selection-aware color accessors ────────────────────────────
  // Wrap whatever the user provided so selected items pick up the
  // accent color. We read the accent from the theme prop with a
  // sensible default. Set membership is O(1) in the selection lists.
  const accentColor = theme?.accent ?? "#4f8ef7";
  const selectedNodeSet = useMemo(
    () => new Set(selection.selected),
    [selection.selected],
  );
  const selectedLinkSet = useMemo(
    () => new Set(selectedLinkIds),
    [selectedLinkIds],
  );

  const wrappedNodeColor = useMemo(() => {
    const base = props.nodeColor;
    return (node: N) => {
      if (node.id !== undefined && selectedNodeSet.has(node.id)) {
        return accentColor;
      }
      if (typeof base === "function") return base(node);
      if (typeof base === "string") return base;
      // Fall back to per-node color, then a default engine handles.
      return (node.color as string | undefined) ?? "#888";
    };
  }, [props.nodeColor, selectedNodeSet, accentColor]);

  const wrappedLinkColor = useMemo(() => {
    const base = props.linkColor;
    return (link: L) => {
      const lid = link.id;
      if (lid !== undefined && selectedLinkSet.has(lid)) return accentColor;
      if (typeof base === "function") return base(link);
      if (typeof base === "string") return base;
      return (link.color as string | undefined) ?? "rgba(0,0,0,0.25)";
    };
  }, [props.linkColor, selectedLinkSet, accentColor]);

  const wrappedLinkWidth = useMemo(() => {
    const base = props.linkWidth;
    return (link: L) => {
      const lid = link.id;
      const isSelected = lid !== undefined && selectedLinkSet.has(lid);
      const baseWidth =
        typeof base === "function"
          ? base(link)
          : typeof base === "number"
            ? base
            : (link.width as number | undefined) ?? 1;
      return isSelected ? baseWidth + 1.5 : baseWidth;
    };
  }, [props.linkWidth, selectedLinkSet]);

  // Build the prop bag that flows to the engine — we override the
  // event hooks so the toolbar / selection / context-menu state stay
  // consistent with what the engine sees, and swap in the wrapped
  // color accessors so selection is visible on the canvas.
  const engineProps = useMemo<LoraGraphCanvasProps<N, L>>(
    () => ({
      ...props,
      nodeColor: wrappedNodeColor,
      linkColor: wrappedLinkColor,
      linkWidth: wrappedLinkWidth,
      // Generous hit area for thin links — without this, edges are
      // hard to click. Hosts can override by passing their own value.
      linkHoverPrecision: props.linkHoverPrecision ?? 8,
      onNodeClick: handleNodeClick,
      onNodeRightClick: handleNodeRightClick,
      onLinkClick: handleLinkClick,
      onLinkRightClick: handleLinkRightClick,
      onBackgroundClick: handleBackgroundClick,
      onBackgroundRightClick: handleBackgroundRightClick,
      onNodeDrag: handleNodeDrag,
      onNodeDragEnd: handleNodeDragEnd,
      // Swallow the engine's HTML tooltip — we render our own. The
      // kapsule reads `nodeLabel` to decide what to show in the
      // tooltip; pass an empty string to hide it.
      nodeLabel: () => "",
      linkLabel: () => "",
      onNodeHover: handleNodeHover,
      onLinkHover: handleLinkHover,
    }),
    [
      props,
      wrappedNodeColor,
      wrappedLinkColor,
      wrappedLinkWidth,
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
    ],
  );

  const engine = useGraphEngine<N, L>({
    mount: mountRef.current,
    mode,
    width,
    height,
    data: dataApi.data,
    props: engineProps,
  });
  engineRef.current = engine;

  // ─── Toolbar dispatch ────────────────────────────────────────────
  // ─── Clipboard primitives ───────────────────────────────────────
  // `copy` and `cut` snapshot the selected nodes into a private,
  // component-scoped clipboard (we do not touch the OS clipboard so
  // the user's other apps aren't affected). `paste` regenerates ids
  // and places the new nodes near the cursor.
  const snapshotSelection = useCallback((): Array<Partial<N>> => {
    const idSet = new Set(selection.selected);
    const out: Array<Partial<N>> = [];
    for (const node of dataApi.data.nodes) {
      if (!idSet.has(node.id)) continue;
      // Strip id + simulation fields so paste generates fresh ones.
      // eslint-disable-next-line @typescript-eslint/no-unused-vars
      const { id, x, y, z, vx, vy, vz, fx, fy, fz, ...rest } =
        node as N & Record<string, unknown>;
      out.push(rest as unknown as Partial<N>);
    }
    return out;
  }, [dataApi.data.nodes, selection.selected]);

  const copySelectionInternal = useCallback((): N[] => {
    if (!enableClipboard) return [];
    const idSet = new Set(selection.selected);
    const snapshot = dataApi.data.nodes.filter((n) => idSet.has(n.id));
    clipboardRef.current = snapshotSelection();
    onCopy?.(snapshot);
    return snapshot;
  }, [
    enableClipboard,
    dataApi.data.nodes,
    selection.selected,
    snapshotSelection,
    onCopy,
  ]);

  const cutSelectionInternal = useCallback((): N[] => {
    if (!enableClipboard) return [];
    const idSet = new Set(selection.selected);
    const snapshot = dataApi.data.nodes.filter((n) => idSet.has(n.id));
    clipboardRef.current = snapshotSelection();
    onCut?.(snapshot);
    if (selection.selected.length > 0) {
      dataApi.removeNodes(selection.selected);
      selection.clear();
    }
    return snapshot;
  }, [
    enableClipboard,
    dataApi,
    selection,
    snapshotSelection,
    onCut,
  ]);

  const pasteFromClipboard = useCallback(
    (at?: { x: number; y: number; z?: number }): N[] => {
      if (!enableClipboard) return [];
      const clipboard = clipboardRef.current;
      if (clipboard.length === 0) return [];
      const target = at
        ? at
        : (() => {
            const c = lastCursorRef.current;
            if (!c || !engineRef.current) return undefined;
            return engineRef.current.screen2Graph(c.x, c.y);
          })();
      const created = dataApi.addNodes(
        clipboard.map((tmpl, i) => {
          const offsetX = (i % 3) * 24;
          const offsetY = Math.floor(i / 3) * 24;
          return {
            ...tmpl,
            ...(target
              ? {
                  x: target.x + offsetX,
                  y: target.y + offsetY,
                  ...(target.z !== undefined
                    ? { z: target.z + offsetY }
                    : {}),
                }
              : {}),
          } as Partial<N> & { id?: string | number };
        }),
      );
      selection.set(created.map((n) => n.id));
      setSelectedLinkIds([]);
      onPaste?.(created);
      return created;
    },
    [enableClipboard, dataApi, selection, onPaste],
  );

  // Duplicate is a self-contained primitive — it doesn't touch the
  // clipboard, so it works even when `enableClipboard` is false.
  const duplicateSelection = useCallback((): N[] => {
    const idSet = new Set(selection.selected);
    const templates: Array<Partial<N> & { id?: string | number }> = [];
    let i = 0;
    for (const node of dataApi.data.nodes) {
      if (!idSet.has(node.id)) continue;
      // eslint-disable-next-line @typescript-eslint/no-unused-vars
      const { id, vx, vy, vz, fx, fy, fz, ...rest } =
        node as N & Record<string, unknown>;
      const offsetX = (i % 3) * 24;
      const offsetY = Math.floor(i / 3) * 24;
      templates.push({
        ...(rest as Partial<N>),
        ...(node.x !== undefined ? { x: node.x + offsetX } : {}),
        ...(node.y !== undefined ? { y: node.y + offsetY } : {}),
        ...(node.z !== undefined ? { z: node.z } : {}),
      });
      i++;
    }
    if (templates.length === 0) return [];
    const created = dataApi.addNodes(templates);
    selection.set(created.map((n) => n.id));
    setSelectedLinkIds([]);
    return created;
  }, [dataApi, selection]);

  // ─── JSON import / export ───────────────────────────────────────
  const fileInputRef = useRef<HTMLInputElement | null>(null);
  const exportJSON = useCallback(
    () =>
      JSON.stringify(
        {
          nodes: dataApi.data.nodes.map((n) => {
            // Drop ephemeral simulation fields so re-imports get fresh
            // layout values.
            // eslint-disable-next-line @typescript-eslint/no-unused-vars
            const { vx, vy, vz, index, ...rest } =
              n as N & Record<string, unknown>;
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
      const parsed = JSON.parse(json) as {
        nodes: N[];
        links: L[];
      };
      if (!parsed || !Array.isArray(parsed.nodes) || !Array.isArray(parsed.links)) {
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
      const url = URL.createObjectURL(blob);
      const a = document.createElement("a");
      a.href = url;
      a.download = filename;
      document.body.appendChild(a);
      a.click();
      document.body.removeChild(a);
      URL.revokeObjectURL(url);
    },
    [exportJSON],
  );

  // ─── Pin/unpin ──────────────────────────────────────────────────
  const togglePin = useCallback(
    (id: string | number) => {
      const node = dataApi.data.nodes.find((n) => n.id === id);
      if (!node) return;
      if (node.fx !== undefined) {
        dataApi.updateNode(id, {
          fx: undefined,
          fy: undefined,
          fz: undefined,
        } as unknown as Partial<N>);
      } else {
        dataApi.updateNode(id, {
          fx: node.x,
          fy: node.y,
          ...(node.z !== undefined ? { fz: node.z } : {}),
        } as unknown as Partial<N>);
      }
    },
    [dataApi],
  );

  // ─── Inline rename ──────────────────────────────────────────────
  const beginRename = useCallback(
    (node: N) => {
      const id = node.id;
      if (id === undefined) return;
      const sc =
        engineRef.current?.graph2Screen(
          node.x ?? 0,
          node.y ?? 0,
          node.z,
        ) ?? { x: 0, y: 0 };
      setRenaming({
        id,
        x: sc.x,
        y: sc.y,
        value: (node.label as string | undefined) ?? String(id),
        previous: node.label as string | undefined,
      });
    },
    [],
  );

  const commitRename = useCallback(() => {
    if (!renaming) return;
    const { id, value, previous } = renaming;
    dataApi.updateNode(id, { label: value } as Partial<N>);
    const updated = dataApi.data.nodes.find((n) => n.id === id);
    if (updated) onNodeRename?.(updated, value, previous);
    setRenaming(null);
  }, [renaming, dataApi, onNodeRename]);

  // Keep the forward-ref pointed at the latest beginRename so
  // handleNodeClick can call it via the trampoline.
  beginRenameRef.current = beginRename;

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
          if (selection.selected.length > 0) {
            dataApi.removeNodes(selection.selected);
            selection.clear();
          }
          if (selectedLinkIds.length > 0) {
            const linkIdSet = new Set(selectedLinkIds);
            dataApi.removeLink((l) =>
              l.id !== undefined && linkIdSet.has(l.id),
            );
            setSelectedLinkIds([]);
          }
          break;
        case "duplicate":
          duplicateSelection();
          break;
        case "select-all":
          selection.set(dataApi.data.nodes.map((n) => n.id));
          setSelectedLinkIds([]);
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
          engine?.pause();
          setPaused(true);
          break;
        case "resume":
          engine?.resume();
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
      duplicateSelection,
      downloadJSON,
    ],
  );

  // ─── Keybindings ─────────────────────────────────────────────────
  useEffect(() => {
    const host = hostRef.current;
    if (!host) return;
    const onKey = (e: KeyboardEvent) => {
      // Only handle when the focus is inside our host or the body.
      const target = e.target as HTMLElement | null;
      const editable =
        target &&
        (target.tagName === "INPUT" ||
          target.tagName === "TEXTAREA" ||
          target.isContentEditable);
      if (editable) return;

      switch (e.key) {
        case "v":
        case "V":
          if (enableClipboard && (e.metaKey || e.ctrlKey)) {
            pasteFromClipboard();
            e.preventDefault();
          } else {
            setActiveTool("select");
          }
          break;
        case "h":
        case "H":
          setActiveTool("pan");
          break;
        case "n":
        case "N":
          setActiveTool("add-node");
          break;
        case "l":
        case "L":
          setActiveTool("add-link");
          break;
        case "f":
        case "F":
          engine?.fit(400, 40);
          break;
        case "3":
          setMode(mode === "2d" ? "3d" : "2d");
          break;
        case "Backspace":
        case "Delete":
          if (selection.selected.length > 0) {
            dataApi.removeNodes(selection.selected);
            selection.clear();
            e.preventDefault();
          }
          if (selectedLinkIds.length > 0) {
            const linkIdSet = new Set(selectedLinkIds);
            dataApi.removeLink((l) =>
              l.id !== undefined && linkIdSet.has(l.id),
            );
            setSelectedLinkIds([]);
            e.preventDefault();
          }
          break;
        case "a":
        case "A":
          if (e.metaKey || e.ctrlKey) {
            selection.set(dataApi.data.nodes.map((n) => n.id));
            setSelectedLinkIds([]);
            e.preventDefault();
          }
          break;
        case "c":
        case "C":
          if (enableClipboard && (e.metaKey || e.ctrlKey)) {
            copySelectionInternal();
            // Let the OS clipboard event fire too — the user might
            // be copying text from a tooltip or similar.
          }
          break;
        case "x":
        case "X":
          if (enableClipboard && (e.metaKey || e.ctrlKey)) {
            cutSelectionInternal();
            e.preventDefault();
          }
          break;
        case "d":
        case "D":
          if (e.metaKey || e.ctrlKey) {
            duplicateSelection();
            e.preventDefault();
          }
          break;
        case "p":
        case "P":
          for (const id of selection.selected) togglePin(id);
          break;
        case "Escape":
          selection.clear();
          setSelectedLinkIds([]);
          setLinkSourceId(null);
          setRenaming(null);
          break;
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [
    engine,
    dataApi,
    selection,
    mode,
    setMode,
    selectedLinkIds,
    duplicateSelection,
    togglePin,
    enableClipboard,
    copySelectionInternal,
    cutSelectionInternal,
    pasteFromClipboard,
  ]);

  // ─── Marquee + cursor tracking on the mount element ─────────────
  //
  // Shift+drag on the canvas draws a selection rectangle and selects
  // every node whose projected screen coordinates fall inside it on
  // release. The kapsule's d3-zoom owns the canvas mouse events; we
  // attach in the capture phase so we can intercept before it. To
  // avoid stealing all background clicks (which would break the
  // add-node tool, panning, etc), the marquee only activates when
  // Shift is held at mousedown.
  //
  // We also use this effect to track the last cursor position over
  // the mount — used by paste-at-cursor and inline rename position.
  useEffect(() => {
    const mount = mountRef.current;
    if (!mount) return;

    const onMouseMove = (e: MouseEvent) => {
      const rect = mount.getBoundingClientRect();
      lastCursorRef.current = {
        x: e.clientX - rect.left,
        y: e.clientY - rect.top,
      };
    };
    mount.addEventListener("mousemove", onMouseMove);

    // Update lastCursorRef on every move so paste-at-cursor and
    // double-click rename can position correctly.
    void lastCursorRef;

    const onMouseDown = (e: MouseEvent) => {
      // Only intercept on left button + shift held.
      if (e.button !== 0 || !e.shiftKey) return;
      const rect = mount.getBoundingClientRect();
      const x0 = e.clientX - rect.left;
      const y0 = e.clientY - rect.top;
      // Block kapsule's d3-zoom from starting a pan.
      e.stopPropagation();
      e.preventDefault();
      setMarquee({ x0, y0, x1: x0, y1: y0, additive: e.shiftKey });

      const onMove = (ev: MouseEvent) => {
        const r = mount.getBoundingClientRect();
        const x1 = ev.clientX - r.left;
        const y1 = ev.clientY - r.top;
        setMarquee((cur) => (cur ? { ...cur, x1, y1 } : cur));
      };
      const onUp = (ev: MouseEvent) => {
        window.removeEventListener("mousemove", onMove);
        window.removeEventListener("mouseup", onUp, true);
        const r = mount.getBoundingClientRect();
        const x1 = ev.clientX - r.left;
        const y1 = ev.clientY - r.top;
        setMarquee(null);
        // Compute selection. Use engineRef so we always read the
        // latest engine (the trampoline pattern); engine may have
        // remounted mid-gesture if the user swapped 2D↔3D, in which
        // case we just bail.
        const eng = engineRef.current;
        if (!eng) return;
        const xMin = Math.min(x0, x1);
        const yMin = Math.min(y0, y1);
        const xMax = Math.max(x0, x1);
        const yMax = Math.max(y0, y1);
        // Tiny boxes count as a "click" — treat as clearing selection.
        if (xMax - xMin < 3 && yMax - yMin < 3) {
          selection.clear();
          setSelectedLinkIds([]);
          return;
        }
        const hits: Array<string | number> = [];
        for (const node of dataApi.data.nodes) {
          if (node.x === undefined || node.y === undefined) continue;
          const sc = eng.graph2Screen(node.x, node.y, node.z);
          if (sc.x >= xMin && sc.x <= xMax && sc.y >= yMin && sc.y <= yMax) {
            hits.push(node.id);
          }
        }
        if (ev.shiftKey || ev.metaKey || ev.ctrlKey) {
          // Additive: union with existing selection.
          const union = new Set<string | number>([
            ...selection.selected,
            ...hits,
          ]);
          selection.set(Array.from(union));
        } else {
          selection.set(hits);
        }
        setSelectedLinkIds([]);
      };
      window.addEventListener("mousemove", onMove);
      // Capture-phase so we beat d3-zoom's mouseup, which clears its
      // own internal state.
      window.addEventListener("mouseup", onUp, true);
    };

    // Capture-phase listener so we run before d3-zoom's handler.
    mount.addEventListener("mousedown", onMouseDown, true);

    return () => {
      mount.removeEventListener("mousemove", onMouseMove);
      mount.removeEventListener("mousedown", onMouseDown, true);
    };
  }, [selection, dataApi.data.nodes]);

  // ─── Imperative handle ──────────────────────────────────────────
  useImperativeHandle(
    ref,
    () => ({
      getData: () => dataApi.data,
      setData: dataApi.setData,
      addNode: dataApi.addNode,
      addNodes: dataApi.addNodes,
      updateNode: dataApi.updateNode,
      removeNode: dataApi.removeNode,
      removeNodes: dataApi.removeNodes,
      addLink: dataApi.addLink,
      addLinks: dataApi.addLinks,
      removeLink: dataApi.removeLink,
      clear: dataApi.clear,

      getSelection: () => selection.selected,
      setSelection: selection.set,
      selectAll: () => selection.set(dataApi.data.nodes.map((n) => n.id)),
      clearSelection: selection.clear,

      getMode: () => mode,
      setMode,
      fit: (durationMs, padding) => engine?.fit(durationMs, padding),
      centerAt: (x, y, z, durationMs) =>
        engine?.centerAt(x, y, z, durationMs),
      zoom: (scale, durationMs) => engine?.zoom(scale, durationMs),
      zoomIn: (step = 1.2) => {
        if (engine) engine.zoom((engine.getZoom?.() ?? 1) * step, 200);
      },
      zoomOut: (step = 1.2) => {
        if (engine) engine.zoom((engine.getZoom?.() ?? 1) / step, 200);
      },

      pause: () => {
        engine?.pause();
        setPaused(true);
      },
      resume: () => {
        engine?.resume();
        setPaused(false);
      },
      reheat: () => engine?.reheat(),
      screenshot: async () => {
        const canvas = engine?.getCanvasElement();
        if (!canvas) return null;
        return new Promise<Blob | null>((resolve) =>
          canvas.toBlob((b) => resolve(b)),
        );
      },

      copy: copySelectionInternal,
      cut: cutSelectionInternal,
      paste: (opts) => pasteFromClipboard(opts?.at),
      duplicate: duplicateSelection,

      renameNode: (id, label) => {
        const prev = dataApi.data.nodes.find((n) => n.id === id);
        dataApi.updateNode(id, { label } as Partial<N>);
        const updated = dataApi.data.nodes.find((n) => n.id === id);
        if (updated)
          onNodeRename?.(
            updated,
            label,
            prev?.label as string | undefined,
          );
      },
      togglePin,

      exportJSON,
      importJSON,
      downloadJSON,

      engine2D: () => (engine?.mode === "2d" ? engine : null),
      engine3D: () => (engine?.mode === "3d" ? engine : null),
    }),
    [
      dataApi,
      engine,
      mode,
      setMode,
      selection,
      copySelectionInternal,
      cutSelectionInternal,
      pasteFromClipboard,
      duplicateSelection,
      togglePin,
      exportJSON,
      importJSON,
      downloadJSON,
      onNodeRename,
    ],
  );

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

  return (
    <div
      ref={hostRef}
      className={["lora-graph-canvas", className ?? ""].join(" ").trim()}
      style={hostStyle}
      data-mode={mode}
      data-tool={activeTool}
    >
      <div ref={mountRef} className="lgc-engine-mount" />
      <GraphToolbar
        config={tools}
        activeTool={activeTool}
        paused={paused}
        mode={mode}
        onSelect={handleToolSelect}
      />
      {menu ? (
        <ContextMenu
          x={menu.x}
          y={menu.y}
          items={menu.items}
          onClose={() => setMenu(null)}
        />
      ) : null}
      <HoverTooltip content={tooltipContent} hostRef={hostRef} />
      <MarqueeOverlay rect={marquee} />
      <SelectionPanel
        nodeCount={selection.selected.length}
        linkCount={selectedLinkIds.length}
        hasClipboard={clipboardRef.current.length > 0}
        enableClipboard={enableClipboard}
        onDelete={() => {
          if (selection.selected.length > 0) {
            dataApi.removeNodes(selection.selected);
            selection.clear();
          }
          if (selectedLinkIds.length > 0) {
            const linkIdSet = new Set(selectedLinkIds);
            dataApi.removeLink(
              (l) => l.id !== undefined && linkIdSet.has(l.id),
            );
            setSelectedLinkIds([]);
          }
        }}
        onDuplicate={() => duplicateSelection()}
        onCopy={() => copySelectionInternal()}
        onCut={() => cutSelectionInternal()}
        onPaste={() => pasteFromClipboard()}
        onClear={() => {
          selection.clear();
          setSelectedLinkIds([]);
        }}
      />
      {renaming ? (
        <input
          className="lgc-rename-input"
          style={{ left: renaming.x, top: renaming.y }}
          autoFocus
          value={renaming.value}
          onChange={(e) =>
            setRenaming((cur) =>
              cur ? { ...cur, value: e.target.value } : cur,
            )
          }
          onKeyDown={(e) => {
            if (e.key === "Enter") {
              commitRename();
              e.preventDefault();
            } else if (e.key === "Escape") {
              setRenaming(null);
              e.preventDefault();
            }
          }}
          onBlur={commitRename}
        />
      ) : null}
      <input
        ref={fileInputRef}
        type="file"
        accept="application/json,.json"
        style={{ display: "none" }}
        onChange={(e) => {
          const file = e.target.files?.[0];
          if (!file) return;
          file.text().then(importJSON).catch((err) => {
            console.error("[lora-graph-canvas] import failed:", err);
          });
          // Reset so picking the same file twice re-fires onChange.
          e.target.value = "";
        }}
      />
    </div>
  );
}

function downloadScreenshot(canvas: HTMLCanvasElement | null | undefined) {
  if (!canvas) return;
  canvas.toBlob((blob) => {
    if (!blob) return;
    const url = URL.createObjectURL(blob);
    const a = document.createElement("a");
    a.href = url;
    a.download = `lora-graph-${Date.now()}.png`;
    document.body.appendChild(a);
    a.click();
    document.body.removeChild(a);
    URL.revokeObjectURL(url);
  });
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
