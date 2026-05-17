"use client";

/**
 * Horizontal strip of editor tabs. Mirrors VS Code: active tab has an
 * underline, dirty tabs show a small dot, middle-click or context-menu
 * closes a tab, and a `+` button at the end opens a fresh one. Tabs can be
 * reordered by dragging — the new order is persisted via the session
 * subscription that reads `tabs` from the store.
 */

import type { DragEvent, MouseEvent } from "react";
import { useState } from "react";
import { ActionIcon, Group, ScrollArea, Text, UnstyledButton } from "@mantine/core";
import { IconPlus, IconX, IconFileCode } from "@tabler/icons-react";

import { requestCloseTab } from "@/lib/actions/tabActions";
import { useStore } from "@/lib/state/store";
import { usePlaygroundTheme } from "@/lib/theme/usePlaygroundTheme";
import { hexA } from "@/lib/theme/util";

const DRAG_MIME = "application/x-loradb-tab";

export function EditorTabs() {
  const { tokens } = usePlaygroundTheme();
  const tabs = useStore((s) => s.tabs);
  const activeId = useStore((s) => s.activeTabId);
  const setActiveTab = useStore((s) => s.setActiveTab);
  const openTab = useStore((s) => s.openTab);
  const reorderTab = useStore((s) => s.reorderTab);
  const [hoverId, setHoverId] = useState<string | null>(null);
  const [draggingId, setDraggingId] = useState<string | null>(null);
  // `dropIndex` is the slot the drop will target — 0..tabs.length. The
  // indicator is drawn on the *left* of the tab at that index, except for
  // tabs.length which renders on the right edge of the last tab.
  const [dropIndex, setDropIndex] = useState<number | null>(null);

  const handleClose = (id: string, e: MouseEvent) => {
    e.stopPropagation();
    requestCloseTab(id);
  };

  const handleAuxClick = (id: string, e: MouseEvent) => {
    if (e.button === 1) {
      e.preventDefault();
      requestCloseTab(id);
    }
  };

  const handleDragStart = (id: string, e: DragEvent<HTMLElement>) => {
    setDraggingId(id);
    // Set both a custom MIME and a text fallback so DataTransfer is non-empty
    // (Firefox refuses to start a drag without payload).
    e.dataTransfer.setData(DRAG_MIME, id);
    e.dataTransfer.setData("text/plain", id);
    e.dataTransfer.effectAllowed = "move";
  };

  const handleDragOver = (
    index: number,
    e: DragEvent<HTMLElement>,
  ) => {
    if (draggingId === null) return;
    e.preventDefault();
    e.dataTransfer.dropEffect = "move";
    const rect = e.currentTarget.getBoundingClientRect();
    const afterHalf = e.clientX - rect.left > rect.width / 2;
    const target = afterHalf ? index + 1 : index;
    setDropIndex((prev) => (prev === target ? prev : target));
  };

  const handleDragLeaveStrip = (e: DragEvent<HTMLElement>) => {
    // Only clear when the cursor actually leaves the strip — leaving a child
    // tab still keeps us over the parent group.
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
    const from = tabs.findIndex((t) => t.id === draggingId);
    if (from === -1) {
      setDraggingId(null);
      setDropIndex(null);
      return;
    }
    // Inserting after the dragged tab's current slot means the array splice
    // shifts the target back by one.
    const to = dropIndex > from ? dropIndex - 1 : dropIndex;
    reorderTab(from, to);
    setDraggingId(null);
    setDropIndex(null);
  };

  const handleDragEnd = () => {
    setDraggingId(null);
    setDropIndex(null);
  };

  return (
    <ScrollArea
      type="hover"
      scrollbars="x"
      offsetScrollbars={false}
      scrollbarSize={6}
      styles={{
        viewport: {
          // Keep the tab row at a fixed height; the viewport must let the
          // inner Group lay out as a single flex row instead of stacking.
          height: 36,
        },
      }}
      style={{
        height: 36,
        flexShrink: 0,
        background: tokens.bg.sidebar,
        borderBottom: `1px solid ${tokens.border.subtle}`,
      }}
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
            onClick={() => setActiveTab(tab.id)}
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
                ? tokens.bg.editor
                : hovered
                  ? hexA(tokens.fg.primary, 0.04)
                  : "transparent",
              color: active ? tokens.fg.primary : tokens.fg.muted,
              minWidth: 120,
              maxWidth: 220,
              flexShrink: 0,
              cursor: dragging ? "grabbing" : "pointer",
              opacity: dragging ? 0.4 : 1,
              borderTop: active
                ? `2px solid ${tokens.accent.primary}`
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
          </UnstyledButton>
        );
      })}
      <ActionIcon
        variant="subtle"
        color="gray"
        size="sm"
        onClick={() => openTab()}
        aria-label="New query tab"
        style={{ alignSelf: "center", marginLeft: 8, marginRight: 6 }}
      >
        <IconPlus size={14} />
      </ActionIcon>
      </Group>
    </ScrollArea>
  );
}
