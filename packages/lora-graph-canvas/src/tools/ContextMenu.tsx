import { useEffect, useRef, type ReactNode } from "react";

export interface ContextMenuItem {
  id: string;
  label: ReactNode;
  shortcut?: string;
  disabled?: boolean;
  onSelect(): void;
}

export interface ContextMenuProps {
  x: number;
  y: number;
  items: Array<ContextMenuItem | { separator: true }>;
  onClose(): void;
}

export function ContextMenu({ x, y, items, onClose }: ContextMenuProps) {
  const ref = useRef<HTMLDivElement | null>(null);

  useEffect(() => {
    const onAny = (e: MouseEvent | KeyboardEvent) => {
      if (e instanceof KeyboardEvent && e.key !== "Escape") return;
      if (
        e instanceof MouseEvent &&
        ref.current &&
        ref.current.contains(e.target as Node)
      ) {
        return;
      }
      onClose();
    };
    window.addEventListener("mousedown", onAny);
    window.addEventListener("keydown", onAny);
    return () => {
      window.removeEventListener("mousedown", onAny);
      window.removeEventListener("keydown", onAny);
    };
  }, [onClose]);

  return (
    <div
      ref={ref}
      className="lgc-menu"
      role="menu"
      style={{ left: x, top: y }}
    >
      {items.map((item, i) =>
        "separator" in item ? (
          <div key={`sep-${i}`} className="lgc-menu-separator" />
        ) : (
          <div
            key={item.id}
            role="menuitem"
            className="lgc-menu-item"
            aria-disabled={item.disabled ? "true" : undefined}
            onClick={() => {
              if (item.disabled) return;
              item.onSelect();
              onClose();
            }}
          >
            <span>{item.label}</span>
            {item.shortcut ? (
              <span style={{ marginLeft: "auto", opacity: 0.6 }}>
                {item.shortcut}
              </span>
            ) : null}
          </div>
        ),
      )}
    </div>
  );
}
