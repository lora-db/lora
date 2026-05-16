import { toggleModeIcon } from "./tools";

export interface ModeToggleProps {
  mode: "2d" | "3d";
  onToggle(): void;
}

/** Floating bottom-right pill that switches between 2D and 3D modes.
 *  Pulled out of the main toolbar so it stays out of the way and is
 *  always findable. Hosts who want the toggle inline can add
 *  `"toggle-mode"` to their `tools` array — the inline button keeps
 *  working alongside this one. */
export function ModeToggle({ mode, onToggle }: ModeToggleProps) {
  const Icon = toggleModeIcon(mode);
  const next = mode === "2d" ? "3D" : "2D";
  return (
    <button
      type="button"
      className="lgc-mode-toggle"
      onClick={onToggle}
      aria-label={`Switch to ${next}`}
      title={`Switch to ${next} (3)`}
    >
      <Icon width={14} height={14} />
      <span className="lgc-mode-toggle-label">{mode.toUpperCase()}</span>
    </button>
  );
}
