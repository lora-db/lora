import { useEffect, useRef, useState } from "react";
import type { ToolId, ToolbarConfig } from "../types";
import { DEFAULT_TOOL_ORDER, TOOL_DESCRIPTORS, toggleModeIcon } from "./tools";

export interface GraphToolbarProps {
  /** Tools the user configured. Falsy → none; `true` → defaults;
   *  array → that exact order; object → include/exclude + position. */
  config: boolean | ToolId[] | ToolbarConfig;
  /** Currently active *toggleable* tool (the cursor mode). */
  activeTool: ToolId;
  /** Engine paused state, so we can swap pause↔resume into one slot. */
  paused: boolean;
  /** Current view mode, so the toggle-mode button can show the right
   *  icon and the right tooltip. */
  mode: "2d" | "3d";
  /** Optional predicate to grey out individual tools (e.g. undo / redo
   *  when the corresponding stack is empty). */
  isDisabled?: (id: ToolId) => boolean;
  onSelect(id: ToolId): void;
}

/** Decide which tools to actually render, in what order. */
function resolveToolList(
  config: GraphToolbarProps["config"],
  paused: boolean,
): ToolId[] {
  if (config === false) return [];
  let list: ToolId[];
  if (config === true) list = DEFAULT_TOOL_ORDER;
  else if (Array.isArray(config)) list = config;
  else {
    const include = config.include ?? DEFAULT_TOOL_ORDER;
    const exclude = new Set(config.exclude ?? []);
    list = include.filter((id) => !exclude.has(id));
  }
  // Pause/resume share a slot: hide whichever isn't currently usable.
  return list.filter((id) => {
    if (id === "pause") return !paused;
    if (id === "resume") return paused;
    return true;
  });
}

function resolvePosition(
  config: GraphToolbarProps["config"],
): "top" | "top-right" | "top-left" | "bottom" {
  if (config && typeof config === "object" && !Array.isArray(config)) {
    return config.position ?? "top-right";
  }
  return "top-right";
}

export function GraphToolbar(props: GraphToolbarProps) {
  const { config, activeTool, paused, mode, isDisabled, onSelect } = props;
  const ids = resolveToolList(config, paused);
  // WAI-ARIA roving tabIndex: the toolbar is a single tab stop, then
  // arrow keys walk between its buttons. Without this every button
  // is independently tab-stoppable and a Tab press inside the canvas
  // gets eaten before it reaches anything else on the page.
  // The "focused index" survives toolbar re-renders so the user's
  // arrow-nav position is preserved across pause/resume swaps. We
  // clamp on every render in case the active set shrinks below the
  // current index.
  const [focusedIdx, setFocusedIdx] = useState(0);
  const buttonsRef = useRef<Array<HTMLButtonElement | null>>([]);
  // Tracks whether the user is actively keyboard-driving the toolbar
  // so we only steal focus when *they* asked for it (e.g. ArrowRight).
  // Without this, mounting / re-rendering the toolbar would pull
  // focus from whatever the user was working on.
  const userActiveRef = useRef(false);
  useEffect(() => {
    if (!userActiveRef.current) return;
    const safeIdx = Math.min(focusedIdx, Math.max(0, ids.length - 1));
    buttonsRef.current[safeIdx]?.focus();
  }, [focusedIdx, ids.length]);

  if (ids.length === 0) return null;
  const position = resolvePosition(config);
  const clampedFocusedIdx = Math.min(focusedIdx, Math.max(0, ids.length - 1));

  const moveFocus = (delta: number) => {
    userActiveRef.current = true;
    setFocusedIdx((cur) => {
      const next = (cur + delta + ids.length) % ids.length;
      return next;
    });
  };

  return (
    <div className={`lgc-toolbar lgc-toolbar--${position}`} role="toolbar">
      {ids.map((id, idx) => {
        const descriptor = TOOL_DESCRIPTORS[id];
        const Icon =
          id === "toggle-mode" ? toggleModeIcon(mode) : descriptor.icon;
        const isActive = descriptor.toggleable && activeTool === id;
        const disabled = isDisabled?.(id) ?? false;
        const tooltipParts = [
          descriptor.hint,
          descriptor.shortcut ? `(${descriptor.shortcut})` : "",
        ];
        const isFocusable = idx === clampedFocusedIdx;
        return (
          <button
            key={id}
            ref={(el) => {
              buttonsRef.current[idx] = el;
            }}
            type="button"
            className={[
              "lgc-tool",
              isActive ? "lgc-tool--active" : "",
              disabled ? "lgc-tool--disabled" : "",
            ]
              .join(" ")
              .trim()}
            aria-label={descriptor.label}
            aria-pressed={isActive ? "true" : undefined}
            disabled={disabled}
            tabIndex={isFocusable ? 0 : -1}
            title={tooltipParts.filter(Boolean).join(" ")}
            onFocus={() => setFocusedIdx(idx)}
            onClick={() => onSelect(id)}
            onKeyDown={(e) => {
              if (e.key === "ArrowRight") {
                e.preventDefault();
                moveFocus(1);
              } else if (e.key === "ArrowLeft") {
                e.preventDefault();
                moveFocus(-1);
              } else if (e.key === "Home") {
                e.preventDefault();
                userActiveRef.current = true;
                setFocusedIdx(0);
              } else if (e.key === "End") {
                e.preventDefault();
                userActiveRef.current = true;
                setFocusedIdx(ids.length - 1);
              }
            }}
          >
            <Icon />
          </button>
        );
      })}
    </div>
  );
}
