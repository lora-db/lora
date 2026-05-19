"use client";

/**
 * Per-pane editor tabs strip. Renders the tab ids in
 * `view.tabIds` for a single editor `PanelView`, with the same
 * VSCode-like affordances as before (underline active tab, dirty dot,
 * middle-click close, drag-to-reorder, + to add).
 *
 * Two scopes interact here:
 *   - the tabs slice owns the master `tabs` records (name + body + dirty)
 *   - this view's `tabIds` ordering owns which tabs appear in *this*
 *     pane and in what order
 */

import type { DragEvent, MouseEvent, ReactNode } from "react";
import { useMemo, useState } from "react";
import { ActionIcon, Group, ScrollArea, Text, Tooltip, UnstyledButton } from "@mantine/core";
import { IconBraces, IconPlus, IconX, IconFileCode, IconLink } from "@tabler/icons-react";

import { useStore } from "@/lib/state/store";
import type { PanelView } from "@/lib/state/slices/layout";
import { iterCells } from "@/lib/state/workspace/tree";
import { usePlaygroundTheme } from "@/lib/theme/usePlaygroundTheme";
import { hexA } from "@/lib/theme/util";
import { closeTabInView, newTabInView } from "@/lib/actions/tabActions";

const DRAG_MIME = "application/x-loradb-tab";

interface EditorTabsProps {
  view: PanelView;
  paneId: string;
  /**
   * Whether the leaf hosting this tab strip is the currently active
   * pane. Drives the active tab's accent treatment so the tab itself
   * carries the pane-focus indicator instead of a frame around the
   * whole pane.
   */
  isPaneActive?: boolean;
  /**
   * Right-aligned content that lives in the same chrome row as the tab
   * strip — pane-management icons (split, minimize, close) ride here so
   * they don't need a second toolbar.
   */
  trailingActions?: ReactNode;
}

export function EditorTabs({ view, paneId, isPaneActive = false, trailingActions }: EditorTabsProps) {
  const { tokens } = usePlaygroundTheme();
  const allTabs = useStore((s) => s.tabs);
  // Workspace invariant: there must always be at least one query tab.
  // Hide the X button on every tab when there's only one globally so the
  // affordance doesn't suggest a closable target.
  const canCloseTabs = useStore((s) => s.tabs.length > 1);
  // Set of tab ids that are open in another cell's editor view too —
  // surfacing a "linked" icon there tells the user that edits to that
  // tab affect the sibling cell as well. Subscribe to the workspace
  // tree (a stable immer reference) and derive the Set in a memo so we
  // don't return a fresh object from the selector every render.
  const workspace = useStore((s) => s.workspace);
  const sharedTabIds = useMemo(() => {
    const counts = new Map<string, number>();
    for (const cell of iterCells(workspace)) {
      for (const t of cell.editorView.tabIds ?? []) {
        counts.set(t, (counts.get(t) ?? 0) + 1);
      }
    }
    const set = new Set<string>();
    for (const [id, n] of counts) if (n > 1) set.add(id);
    return set;
  }, [workspace]);
  const setActivePane = useStore((s) => s.setActivePane);
  const setViewTabId = useStore((s) => s.setViewTabId);
  const reorderTabInEditorView = useStore((s) => s.reorderTabInEditorView);
  const [hoverId, setHoverId] = useState<string | null>(null);
  const [draggingId, setDraggingId] = useState<string | null>(null);
  const [dropIndex, setDropIndex] = useState<number | null>(null);

  const tabIds = view.tabIds ?? [];
  const activeId = view.tabId ?? null;

  // Resolve ids to records, dropping unknown ids so stale state can't
  // crash the strip.
  const tabs = tabIds
    .map((id) => allTabs.find((t) => t.id === id))
    .filter((t): t is (typeof allTabs)[number] => Boolean(t));

  const handleSelect = (id: string) => {
    setActivePane(paneId);
    setViewTabId(view.id, id);
  };

  const handleClose = (id: string, e: MouseEvent) => {
    e.stopPropagation();
    closeTabInView(view.id, id);
  };

  const handleAuxClick = (id: string, e: MouseEvent) => {
    if (e.button === 1) {
      e.preventDefault();
      closeTabInView(view.id, id);
    }
  };

  const handleDragStart = (id: string, e: DragEvent<HTMLElement>) => {
    setDraggingId(id);
    e.dataTransfer.setData(DRAG_MIME, id);
    e.dataTransfer.setData("text/plain", id);
    e.dataTransfer.effectAllowed = "move";
  };

  const handleDragOver = (index: number, e: DragEvent<HTMLElement>) => {
    if (draggingId === null) return;
    e.preventDefault();
    e.dataTransfer.dropEffect = "move";
    const rect = e.currentTarget.getBoundingClientRect();
    const afterHalf = e.clientX - rect.left > rect.width / 2;
    const target = afterHalf ? index + 1 : index;
    setDropIndex((prev) => (prev === target ? prev : target));
  };

  const handleDragLeaveStrip = (e: DragEvent<HTMLElement>) => {
    if (e.currentTarget.contains(e.relatedTarget as Node | null)) return;
    setDropIndex(null);
  };

  const handleDrop = (e: DragEvent<HTMLElement>) => {
    if (draggingId === null || dropIndex === null) {
      setDraggingId(null);
      setDropIndex(null);
      return;
    }
    e.preventDefault();
    const from = tabIds.indexOf(draggingId);
    if (from === -1) {
      setDraggingId(null);
      setDropIndex(null);
      return;
    }
    const to = dropIndex > from ? dropIndex - 1 : dropIndex;
    reorderTabInEditorView(view.id, from, to);
    setDraggingId(null);
    setDropIndex(null);
  };

  const handleDragEnd = () => {
    setDraggingId(null);
    setDropIndex(null);
  };

  return (
    <div
      style={{
        display: "flex",
        alignItems: "stretch",
        height: 36,
        flexShrink: 0,
        background: tokens.bg.sidebar,
        borderBottom: `1px solid ${tokens.border.subtle}`,
        minWidth: 0,
      }}
    >
      <ScrollArea
        type="hover"
        scrollbars="x"
        offsetScrollbars={false}
        scrollbarSize={6}
        styles={{
          viewport: {
            height: 36,
          },
        }}
        style={{ height: 36, flex: 1, minWidth: 0 }}
      >
      <Group
        gap={0}
        wrap="nowrap"
        align="stretch"
        onDragLeave={handleDragLeaveStrip}
        onDrop={handleDrop}
        style={{ height: 36, minWidth: "max-content" }}
      >
        {tabs.map((tab, index) => {
          const active = tab.id === activeId;
          const hovered = tab.id === hoverId;
          const dragging = tab.id === draggingId;
          const showClose = active || hovered;
          const showIndicatorLeft = dropIndex === index && draggingId !== null;
          const showIndicatorRight =
            dropIndex === tabs.length && index === tabs.length - 1 && draggingId !== null;
          return (
            <UnstyledButton
              key={tab.id}
              draggable
              onDragStart={(e: DragEvent<HTMLElement>) => handleDragStart(tab.id, e)}
              onDragOver={(e: DragEvent<HTMLElement>) => handleDragOver(index, e)}
              onDragEnd={handleDragEnd}
              onClick={() => handleSelect(tab.id)}
              onAuxClick={(e: MouseEvent) => handleAuxClick(tab.id, e)}
              onMouseEnter={() => setHoverId(tab.id)}
              onMouseLeave={() => setHoverId((id) => (id === tab.id ? null : id))}
              data-testid={`editor-tab-${tab.id}`}
              style={{
                position: "relative",
                display: "flex",
                alignItems: "center",
                gap: 8,
                padding: "0 12px 0 14px",
                borderRight: `1px solid ${tokens.border.subtle}`,
                background: active
                  ? isPaneActive
                    ? tokens.bg.editor
                    : hexA(tokens.fg.primary, 0.06)
                  : hovered
                    ? hexA(tokens.fg.primary, 0.04)
                    : "transparent",
                color: active
                  ? isPaneActive
                    ? tokens.fg.primary
                    : tokens.fg.muted
                  : tokens.fg.muted,
                minWidth: 120,
                maxWidth: 220,
                flexShrink: 0,
                cursor: dragging ? "grabbing" : "pointer",
                opacity: dragging ? 0.4 : 1,
                borderTop: active
                  ? isPaneActive
                    ? `2px solid ${tokens.accent.primary}`
                    : `2px solid ${tokens.border.strong}`
                  : "2px solid transparent",
                boxSizing: "border-box",
                transition: "background 0.08s, color 0.08s, opacity 0.08s",
              }}
              aria-label={`Activate tab ${tab.name}`}
            >
              {showIndicatorLeft ? (
                <span
                  aria-hidden
                  style={{
                    position: "absolute",
                    left: -1,
                    top: 0,
                    bottom: 0,
                    width: 2,
                    background: tokens.accent.primary,
                    pointerEvents: "none",
                  }}
                />
              ) : null}
              {showIndicatorRight ? (
                <span
                  aria-hidden
                  style={{
                    position: "absolute",
                    right: -1,
                    top: 0,
                    bottom: 0,
                    width: 2,
                    background: tokens.accent.primary,
                    pointerEvents: "none",
                  }}
                />
              ) : null}
              <IconFileCode
                size={14}
                stroke={1.5}
                style={{ flexShrink: 0, opacity: active ? 0.9 : 0.6 }}
              />
              {sharedTabIds.has(tab.id) && (
                <Tooltip
                  label="Open in another cell — edits sync"
                  openDelay={400}
                  withArrow
                >
                  <IconLink
                    size={10}
                    stroke={1.5}
                    style={{ flexShrink: 0, opacity: 0.7 }}
                    aria-label="Tab is open in another cell"
                  />
                </Tooltip>
              )}
              {tab.params && tab.params.trim() !== "" && tab.params.trim() !== "{}" && (
                <Tooltip
                  label="This tab has bound params"
                  openDelay={400}
                  withArrow
                >
                  <IconBraces
                    size={11}
                    stroke={1.8}
                    style={{
                      flexShrink: 0,
                      color: tokens.accent.primary,
                      opacity: 0.85,
                    }}
                    aria-label="Bound params present"
                  />
                </Tooltip>
              )}
              <Text
                size="xs"
                fw={active ? 500 : 400}
                ff={tokens.font.ui}
                style={{
                  whiteSpace: "nowrap",
                  overflow: "hidden",
                  textOverflow: "ellipsis",
                  flex: 1,
                  textAlign: "left",
                }}
              >
                {tab.name}
              </Text>
              {tab.dirty ? (
                <span
                  aria-label="unsaved"
                  style={{
                    width: 8,
                    height: 8,
                    borderRadius: "50%",
                    background: tokens.fg.primary,
                    display: showClose ? "none" : "inline-block",
                    flexShrink: 0,
                  }}
                />
              ) : null}
              {canCloseTabs && (
                <ActionIcon
                  component="span"
                  role="button"
                  tabIndex={0}
                  variant="subtle"
                  size="xs"
                  color="gray"
                  onClick={(e: MouseEvent) => handleClose(tab.id, e)}
                  aria-label={`Close tab ${tab.name}`}
                  style={{
                    opacity: showClose ? 1 : 0,
                    transition: "opacity 0.08s",
                    flexShrink: 0,
                  }}
                >
                  <IconX size={12} />
                </ActionIcon>
              )}
            </UnstyledButton>
          );
        })}
        <ActionIcon
          variant="subtle"
          color="gray"
          size="sm"
          onClick={() => newTabInView(view.id)}
          aria-label="New query tab"
          style={{ alignSelf: "center", marginLeft: 8, marginRight: 6 }}
        >
          <IconPlus size={14} />
        </ActionIcon>
      </Group>
      </ScrollArea>
      {trailingActions ? (
        <div
          style={{
            display: "flex",
            alignItems: "center",
            flexShrink: 0,
            paddingRight: 4,
            paddingLeft: 4,
            borderLeft: `1px solid ${tokens.border.subtle}`,
          }}
        >
          {trailingActions}
        </div>
      ) : null}
    </div>
  );
}
