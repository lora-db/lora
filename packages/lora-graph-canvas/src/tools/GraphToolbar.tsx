import type { ToolId, ToolbarConfig } from "../types";
import {
  DEFAULT_TOOL_ORDER,
  TOOL_DESCRIPTORS,
  toggleModeIcon,
} from "./tools";

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

function resolvePosition(config: GraphToolbarProps["config"]):
  | "top"
  | "top-right"
  | "top-left"
  | "bottom" {
  if (config && typeof config === "object" && !Array.isArray(config)) {
    return config.position ?? "top-right";
  }
  return "top-right";
}

export function GraphToolbar(props: GraphToolbarProps) {
  const { config, activeTool, paused, mode, isDisabled, onSelect } = props;
  const ids = resolveToolList(config, paused);
  if (ids.length === 0) return null;
  const position = resolvePosition(config);
  return (
    <div className={`lgc-toolbar lgc-toolbar--${position}`} role="toolbar">
      {ids.map((id) => {
        const descriptor = TOOL_DESCRIPTORS[id];
        const Icon =
          id === "toggle-mode" ? toggleModeIcon(mode) : descriptor.icon;
        const isActive = descriptor.toggleable && activeTool === id;
        const disabled = isDisabled?.(id) ?? false;
        const tooltipParts = [
          descriptor.hint,
          descriptor.shortcut ? `(${descriptor.shortcut})` : "",
        ];
        return (
          <button
            key={id}
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
            title={tooltipParts.filter(Boolean).join(" ")}
            onClick={() => onSelect(id)}
          >
            <Icon />
          </button>
        );
      })}
    </div>
  );
}
