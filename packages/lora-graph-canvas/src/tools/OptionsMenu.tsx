import { useEffect, useRef, useState } from "react";

/** A single entry shown inside the menu. Either a boolean checkbox
 *  or a dropdown select. */
export type OptionItem =
  | {
      kind?: "toggle";
      id: string;
      label: string;
      checked: boolean;
      onChange(next: boolean): void;
      hint?: string;
    }
  | {
      kind: "select";
      id: string;
      label: string;
      value: string;
      options: Array<{ value: string; label?: string }>;
      onChange(next: string): void;
      hint?: string;
    };

export interface OptionsMenuProps {
  items: OptionItem[];
}

/** Small inline gear icon. Local to this component since it's not
 *  used by the main toolbar. */
function IconGear(p: React.SVGProps<SVGSVGElement>) {
  return (
    <svg
      width={14}
      height={14}
      viewBox="0 0 16 16"
      fill="none"
      stroke="currentColor"
      strokeWidth={1.5}
      strokeLinecap="round"
      strokeLinejoin="round"
      aria-hidden
      {...p}
    >
      <circle cx="8" cy="8" r="2.4" />
      <path d="M8 1v2M8 13v2M1 8h2M13 8h2M3.05 3.05l1.4 1.4M11.55 11.55l1.4 1.4M3.05 12.95l1.4-1.4M11.55 4.45l1.4-1.4" />
    </svg>
  );
}

/** Floating bottom-right button + popover. The button toggles a small
 *  panel of checkbox-style options. Auto-closes on outside click /
 *  Escape. Renders nothing if `items` is empty. */
export function OptionsMenu({ items }: OptionsMenuProps) {
  const [open, setOpen] = useState(false);
  const wrapperRef = useRef<HTMLDivElement | null>(null);

  useEffect(() => {
    if (!open) return;
    const onDown = (e: MouseEvent) => {
      if (
        wrapperRef.current &&
        !wrapperRef.current.contains(e.target as Node)
      ) {
        setOpen(false);
      }
    };
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") setOpen(false);
    };
    window.addEventListener("mousedown", onDown);
    window.addEventListener("keydown", onKey);
    return () => {
      window.removeEventListener("mousedown", onDown);
      window.removeEventListener("keydown", onKey);
    };
  }, [open]);

  if (items.length === 0) return null;

  return (
    <div ref={wrapperRef} className="lgc-options-menu">
      <button
        type="button"
        className="lgc-options-trigger"
        aria-label="Options"
        aria-expanded={open}
        title="Options"
        onClick={() => setOpen((v) => !v)}
      >
        <IconGear />
      </button>
      {open ? (
        <div className="lgc-options-panel" role="menu">
          {items.map((item) =>
            item.kind === "select" ? (
              <div key={item.id} className="lgc-options-item">
                <span className="lgc-options-item-text">
                  <span className="lgc-options-item-label">{item.label}</span>
                  {item.hint ? (
                    <span className="lgc-options-item-hint">{item.hint}</span>
                  ) : null}
                </span>
                <select
                  className="lgc-options-select"
                  value={item.value}
                  onChange={(e) => item.onChange(e.target.value)}
                >
                  {item.options.map((opt) => (
                    <option key={opt.value} value={opt.value}>
                      {opt.label ?? opt.value}
                    </option>
                  ))}
                </select>
              </div>
            ) : (
              <label key={item.id} className="lgc-options-item">
                <input
                  type="checkbox"
                  checked={item.checked}
                  onChange={(e) => item.onChange(e.target.checked)}
                />
                <span className="lgc-options-item-text">
                  <span className="lgc-options-item-label">{item.label}</span>
                  {item.hint ? (
                    <span className="lgc-options-item-hint">{item.hint}</span>
                  ) : null}
                </span>
              </label>
            ),
          )}
        </div>
      ) : null}
    </div>
  );
}
