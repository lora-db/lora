"use client";

/**
 * Renders the contents of a single leaf — a view-tab strip listing
 * every view inside the leaf plus the active view's content area. Also
 * exposes per-pane controls (split right / split down / close pane)
 * and tracks focus so the store's `activePaneId` follows user gaze.
 */

import { useCallback, useState } from "react";
import { ActionIcon, Group, Menu, Text, Tooltip, UnstyledButton } from "@mantine/core";
import {
  IconBinaryTree,
  IconFileCode,
  IconLayoutColumns,
  IconLayoutRows,
  IconPlus,
  IconX,
} from "@tabler/icons-react";

import { EditorPane } from "@/app/_components/Editor/EditorPane";
import { EditorTabs } from "@/app/_components/Editor/EditorTabs";
import { ResultPane } from "@/app/_components/Result/ResultPane";
import { useStore } from "@/lib/state/store";
import { useTabById } from "@/lib/state/selectors";
import type { PanelLeaf, PanelView } from "@/lib/state/slices/layout";
import {
  closePaneById,
  closeView,
  moveViewToPane,
  openViewInActivePane,
  splitActivePane,
  toggleParentOrientation,
} from "@/lib/actions/workspaceActions";
import { usePlaygroundTheme } from "@/lib/theme/usePlaygroundTheme";
import { hexA } from "@/lib/theme/util";
import {
  countEditorViews,
  countResultViews,
  flatLeafIds,
} from "@/lib/state/workspace/tree";

const VIEW_DRAG_MIME = "application/x-loradb-view";

interface PanelLeafFrameProps {
  leaf: PanelLeaf;
}

export function PanelLeafFrame({ leaf }: PanelLeafFrameProps) {
  const { tokens } = usePlaygroundTheme();
  const isActive = useStore((s) => s.activePaneId === leaf.id);
  const setActivePane = useStore((s) => s.setActivePane);
  // Disable Close-pane when removing this leaf would drop the editor
  // or result count below 1 (workspace invariants).
  const canClose = useStore((s) => {
    if (flatLeafIds(s.workspace).length === 1) return false;
    const closingEditors = leaf.views.filter((v) => v.kind === "editor").length;
    const closingResults = leaf.views.filter((v) => v.kind === "result").length;
    if (closingEditors > 0 && countEditorViews(s.workspace) - closingEditors < 1) return false;
    if (closingResults > 0 && countResultViews(s.workspace) - closingResults < 1) return false;
    return true;
  });

  const view = leaf.views.find((v) => v.id === leaf.activeViewId) ?? leaf.views[0]!;

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
      aria-label={`Pane (${view.kind})`}
      aria-current={isActive ? "true" : undefined}
    >
      <LeafHeader leaf={leaf} canClose={canClose} />
      {view.kind === "editor" && <EditorTabs view={view} paneId={leaf.id} />}
      <div style={{ flex: 1, minHeight: 0, display: "flex", flexDirection: "column" }}>
        {view.kind === "editor" ? (
          <EditorPane tabId={view.tabId} />
        ) : (
          <ResultPane view={view} paneId={leaf.id} />
        )}
      </div>
      {/*
        Active-pane indicator. Rendered as an absolutely-positioned
        overlay (rather than a CSS `outline` on the pane itself) so it
        always paints above sibling separators and panel borders, never
        underneath them. `pointer-events: none` keeps drags and clicks
        flowing through to the content underneath.
      */}
      {isActive && (
        <div
          aria-hidden
          style={{
            position: "absolute",
            inset: 0,
            pointerEvents: "none",
            boxShadow: `inset 0 0 0 1px ${tokens.accent.primary}`,
            zIndex: 5,
          }}
        />
      )}
    </div>
  );
}

function LeafHeader({ leaf, canClose }: { leaf: PanelLeaf; canClose: boolean }) {
  const { tokens } = usePlaygroundTheme();
  const setActiveView = useStore((s) => s.setActiveView);
  const [dragOver, setDragOver] = useState<"none" | "tabs" | "left" | "right" | "top" | "bottom">("none");

  return (
    <div
      onDragOver={(e) => {
        if (!e.dataTransfer.types.includes(VIEW_DRAG_MIME)) return;
        e.preventDefault();
        e.dataTransfer.dropEffect = "move";
      }}
      onDragEnter={(e) => {
        if (e.dataTransfer.types.includes(VIEW_DRAG_MIME)) setDragOver("tabs");
      }}
      onDragLeave={(e) => {
        if (e.currentTarget.contains(e.relatedTarget as Node | null)) return;
        setDragOver("none");
      }}
      onDrop={(e) => {
        const viewId = e.dataTransfer.getData(VIEW_DRAG_MIME);
        if (!viewId) return;
        e.preventDefault();
        moveViewToPane(viewId, leaf.id);
        setDragOver("none");
      }}
      style={{
        display: "flex",
        alignItems: "stretch",
        background: tokens.bg.sidebar,
        borderBottom: `1px solid ${tokens.border.subtle}`,
        minHeight: 28,
        flexShrink: 0,
        position: "relative",
      }}
      data-testid={`leaf-header-${leaf.id}`}
    >
      <Group
        gap={0}
        wrap="nowrap"
        role="tablist"
        aria-label="Pane views"
        style={{ flex: 1, minWidth: 0, height: 28 }}
      >
        {leaf.views.map((v) => (
          <ViewTabChip
            key={v.id}
            view={v}
            leaf={leaf}
            active={v.id === leaf.activeViewId}
            onClick={() => setActiveView(leaf.id, v.id)}
          />
        ))}
      </Group>

      <LeafActions leaf={leaf} canClose={canClose} />

      {dragOver === "tabs" && (
        <div
          aria-hidden
          style={{
            position: "absolute",
            inset: 0,
            background: hexA(tokens.accent.primary, 0.08),
            border: `1px dashed ${tokens.accent.primary}`,
            pointerEvents: "none",
          }}
        />
      )}
    </div>
  );
}

function ViewTabChip({
  view,
  leaf,
  active,
  onClick,
}: {
  view: PanelView;
  leaf: PanelLeaf;
  active: boolean;
  onClick: () => void;
}) {
  const { tokens } = usePlaygroundTheme();
  // Every view (editor or result) now carries an explicit `tabId`
  // pointing at the cell's current tab, so the chip can simply look it
  // up — no follow-active fallback needed.
  const tab = useTabById(view.tabId);
  const [hover, setHover] = useState(false);

  const kindLabel = view.kind === "editor" ? "Editor" : "Result";
  const Icon = view.kind === "editor" ? IconFileCode : IconBinaryTree;
  const moreThanOne = leaf.views.length > 1;

  return (
    <UnstyledButton
      draggable
      onDragStart={(e) => {
        e.dataTransfer.setData(VIEW_DRAG_MIME, view.id);
        e.dataTransfer.setData("text/plain", view.id);
        e.dataTransfer.effectAllowed = "move";
      }}
      onClick={onClick}
      onMouseEnter={() => setHover(true)}
      onMouseLeave={() => setHover(false)}
      role="tab"
      aria-selected={active}
      data-view-id={view.id}
      data-view-kind={view.kind}
      style={{
        display: "flex",
        alignItems: "center",
        gap: 6,
        padding: "0 10px",
        height: 28,
        borderRight: `1px solid ${tokens.border.subtle}`,
        color: active ? tokens.fg.primary : tokens.fg.muted,
        background: active
          ? tokens.bg.editor
          : hover
            ? hexA(tokens.fg.primary, 0.04)
            : "transparent",
        borderTop: active
          ? `2px solid ${tokens.accent.primary}`
          : "2px solid transparent",
        cursor: "pointer",
        fontSize: 11,
        minWidth: 0,
      }}
    >
      <Icon size={12} stroke={1.5} style={{ flexShrink: 0, opacity: active ? 0.9 : 0.6 }} />
      <Text
        size="xs"
        ff={tokens.font.ui}
        style={{
          whiteSpace: "nowrap",
          overflow: "hidden",
          textOverflow: "ellipsis",
          maxWidth: 160,
        }}
      >
        {kindLabel}
        {tab ? ` · ${tab.name}` : ""}
      </Text>
      {moreThanOne && (hover || active) && (
        <ActionIcon
          component="span"
          variant="subtle"
          size="xs"
          color="gray"
          aria-label={`Close ${kindLabel} view`}
          onClick={(e) => {
            e.stopPropagation();
            closeView(view.id);
          }}
        >
          <IconX size={10} />
        </ActionIcon>
      )}
    </UnstyledButton>
  );
}

function LeafActions({ leaf, canClose }: { leaf: PanelLeaf; canClose: boolean }) {
  const { tokens } = usePlaygroundTheme();
  const setActivePane = useStore((s) => s.setActivePane);

  const onSplit = (dir: "row" | "column") => {
    setActivePane(leaf.id);
    splitActivePane(dir, "after");
  };

  return (
    <Group gap={2} align="center" pr={4} pl={4} style={{ height: 28 }}>
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
            <Text size="xs" c={tokens.fg.muted}>
              ⋯
            </Text>
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
          <Menu.Item
            onClick={() => toggleParentOrientation(leaf.id)}
          >
            Toggle parent orientation
          </Menu.Item>
          <Menu.Divider />
          <Menu.Label>Add view</Menu.Label>
          <Menu.Item
            leftSection={<IconPlus size={14} />}
            onClick={() => {
              useStore.getState().setActivePane(leaf.id);
              openViewInActivePane("result");
            }}
          >
            Add result view
          </Menu.Item>
          <Menu.Item
            leftSection={<IconPlus size={14} />}
            onClick={() => {
              useStore.getState().setActivePane(leaf.id);
              openViewInActivePane("editor");
            }}
          >
            Add editor view
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
