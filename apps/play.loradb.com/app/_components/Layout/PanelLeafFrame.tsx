"use client";

/**
 * Renders the contents of a single leaf — one "query" pane that owns
 * its tab strip, editor surface and result region as a single bound
 * unit. The result region can be minimized to a thin restore strip,
 * but it can never be closed independently: closing the leaf disposes
 * of editor + result together.
 */

import { useCallback, useMemo } from "react";
import {
  ActionIcon,
  Group,
  Menu,
  Tooltip,
} from "@mantine/core";
import {
  Group as PanelsGroup,
  type Layout,
  Panel,
  Separator,
} from "react-resizable-panels";
import {
  IconChevronDown,
  IconChevronUp,
  IconLayoutColumns,
  IconLayoutRows,
  IconX,
} from "@tabler/icons-react";

import { EditorPane } from "@/app/_components/Editor/EditorPane";
import { EditorTabs } from "@/app/_components/Editor/EditorTabs";
import { ResultPane } from "@/app/_components/Result/ResultPane";
import { useStore } from "@/lib/state/store";
import type { PanelLeaf, PanelView } from "@/lib/state/slices/layout";
import {
  closePaneById,
  splitActivePane,
  toggleParentOrientation,
} from "@/lib/actions/workspaceActions";
import { usePlaygroundTheme } from "@/lib/theme/usePlaygroundTheme";
import {
  countQueryViews,
  flatLeafIds,
} from "@/lib/state/workspace/tree";

const MINIMIZED_RESULT_PX = 28;

interface PanelLeafFrameProps {
  leaf: PanelLeaf;
}

export function PanelLeafFrame({ leaf }: PanelLeafFrameProps) {
  const { tokens } = usePlaygroundTheme();
  const isActive = useStore((s) => s.activePaneId === leaf.id);
  const setActivePane = useStore((s) => s.setActivePane);
  const canClose = useStore((s) => {
    if (flatLeafIds(s.workspace).length === 1) return false;
    return countQueryViews(s.workspace) - leaf.views.length >= 1;
  });

  const view =
    leaf.views.find((v) => v.id === leaf.activeViewId) ?? leaf.views[0]!;

  const onFocusCapture = useCallback(() => {
    setActivePane(leaf.id);
  }, [leaf.id, setActivePane]);

  return (
    <div
      onFocusCapture={onFocusCapture}
      onMouseDown={onFocusCapture}
      style={{
        flex: 1,
        minHeight: 0,
        minWidth: 0,
        display: "flex",
        flexDirection: "column",
        background: tokens.bg.editor,
        position: "relative",
      }}
      data-pane-id={leaf.id}
      role="region"
      aria-label="Query pane"
      aria-current={isActive ? "true" : undefined}
    >
      <EditorTabs
        view={view}
        paneId={leaf.id}
        isPaneActive={isActive}
        trailingActions={<LeafActions leaf={leaf} view={view} canClose={canClose} />}
      />
      <QueryBody view={view} paneId={leaf.id} />
    </div>
  );
}

function QueryBody({ view, paneId }: { view: PanelView; paneId: string }) {
  const { tokens } = usePlaygroundTheme();
  const minimized = view.resultMinimized ?? false;
  const setEditorSizePctForView = useStore((s) => s.setEditorSizePctForView);
  const editorSizePct = view.editorSizePct ?? 50;

  const defaultLayout = useMemo<Layout>(
    () => ({ editor: editorSizePct, result: 100 - editorSizePct }),
    [editorSizePct],
  );

  const onLayoutChanged = useCallback(
    (layout: Layout) => {
      const next = layout.editor ?? editorSizePct;
      setEditorSizePctForView(view.id, next);
    },
    [editorSizePct, setEditorSizePctForView, view.id],
  );

  if (minimized) {
    return (
      <div
        style={{
          flex: 1,
          minHeight: 0,
          display: "flex",
          flexDirection: "column",
        }}
      >
        <div style={{ flex: 1, minHeight: 0, display: "flex" }}>
          <EditorPane tabId={view.tabId} />
        </div>
        <MinimizedResultStrip view={view} />
      </div>
    );
  }

  return (
    <PanelsGroup
      key={`leaf-${paneId}`}
      id={`leaf-${paneId}`}
      orientation="vertical"
      defaultLayout={defaultLayout}
      onLayoutChanged={onLayoutChanged}
      style={{ flex: 1, minHeight: 0 }}
    >
      <Panel
        id="editor"
        defaultSize={editorSizePct}
        minSize={20}
        style={{
          display: "flex",
          flexDirection: "column",
          minHeight: 0,
        }}
      >
        <EditorPane tabId={view.tabId} />
      </Panel>
      <Separator
        style={{
          height: 4,
          background: tokens.border.subtle,
          cursor: "row-resize",
        }}
      />
      <Panel
        id="result"
        defaultSize={100 - editorSizePct}
        minSize={10}
        style={{
          display: "flex",
          flexDirection: "column",
          minHeight: 0,
        }}
      >
        <ResultPane view={view} paneId={paneId} />
      </Panel>
    </PanelsGroup>
  );
}

function MinimizedResultStrip({ view }: { view: PanelView }) {
  const { tokens } = usePlaygroundTheme();
  const setResultMinimizedForView = useStore((s) => s.setResultMinimizedForView);
  return (
    <button
      type="button"
      onClick={() => setResultMinimizedForView(view.id, false)}
      style={{
        all: "unset",
        cursor: "pointer",
        height: MINIMIZED_RESULT_PX,
        flexShrink: 0,
        display: "flex",
        alignItems: "center",
        justifyContent: "space-between",
        padding: "0 10px",
        background: tokens.bg.sidebar,
        borderTop: `1px solid ${tokens.border.subtle}`,
        color: tokens.fg.muted,
        fontSize: 11,
        fontFamily: tokens.font.ui,
      }}
      aria-label="Restore result region"
    >
      <span>Results minimized</span>
      <IconChevronUp size={14} />
    </button>
  );
}

function LeafActions({
  leaf,
  view,
  canClose,
}: {
  leaf: PanelLeaf;
  view: PanelView;
  canClose: boolean;
}) {
  const setActivePane = useStore((s) => s.setActivePane);
  const setResultMinimizedForView = useStore((s) => s.setResultMinimizedForView);
  const minimized = view.resultMinimized ?? false;

  const onSplit = (dir: "row" | "column") => {
    setActivePane(leaf.id);
    splitActivePane(dir, "after");
  };

  return (
    <Group gap={2} align="center" wrap="nowrap">
      <Tooltip
        label={minimized ? "Restore results" : "Minimize results"}
        openDelay={400}
        withArrow
      >
        <ActionIcon
          variant="subtle"
          color="gray"
          size="sm"
          aria-label={minimized ? "Restore results" : "Minimize results"}
          onClick={() => setResultMinimizedForView(view.id, !minimized)}
        >
          {minimized ? <IconChevronUp size={14} /> : <IconChevronDown size={14} />}
        </ActionIcon>
      </Tooltip>
      <Tooltip label="Split right" openDelay={400} withArrow>
        <ActionIcon
          variant="subtle"
          color="gray"
          size="sm"
          aria-label="Split right"
          onClick={() => onSplit("row")}
        >
          <IconLayoutColumns size={14} />
        </ActionIcon>
      </Tooltip>
      <Tooltip label="Split down" openDelay={400} withArrow>
        <ActionIcon
          variant="subtle"
          color="gray"
          size="sm"
          aria-label="Split down"
          onClick={() => onSplit("column")}
        >
          <IconLayoutRows size={14} />
        </ActionIcon>
      </Tooltip>
      <Menu position="bottom-end" withArrow>
        <Menu.Target>
          <ActionIcon variant="subtle" color="gray" size="sm" aria-label="Pane menu">
            ⋯
          </ActionIcon>
        </Menu.Target>
        <Menu.Dropdown>
          <Menu.Label>Pane</Menu.Label>
          <Menu.Item
            leftSection={<IconLayoutColumns size={14} />}
            onClick={() => onSplit("row")}
          >
            Split right
          </Menu.Item>
          <Menu.Item
            leftSection={<IconLayoutRows size={14} />}
            onClick={() => onSplit("column")}
          >
            Split down
          </Menu.Item>
          <Menu.Item onClick={() => toggleParentOrientation(leaf.id)}>
            Toggle parent orientation
          </Menu.Item>
          <Menu.Divider />
          <Menu.Item
            leftSection={
              minimized ? <IconChevronUp size={14} /> : <IconChevronDown size={14} />
            }
            onClick={() => setResultMinimizedForView(view.id, !minimized)}
          >
            {minimized ? "Restore results" : "Minimize results"}
          </Menu.Item>
          <Menu.Divider />
          <Menu.Item
            color="red"
            leftSection={<IconX size={14} />}
            disabled={!canClose}
            onClick={() => closePaneById(leaf.id)}
          >
            Close pane
          </Menu.Item>
        </Menu.Dropdown>
      </Menu>
      {canClose && (
        <Tooltip label="Close pane" openDelay={400} withArrow>
          <ActionIcon
            variant="subtle"
            color="gray"
            size="sm"
            aria-label="Close pane"
            onClick={() => closePaneById(leaf.id)}
          >
            <IconX size={14} />
          </ActionIcon>
        </Tooltip>
      )}
    </Group>
  );
}
