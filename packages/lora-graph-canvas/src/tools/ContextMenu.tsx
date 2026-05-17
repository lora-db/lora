import { useEffect, useLayoutEffect, useRef, useState, type ReactNode } from "react";

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

/** Margin from the host's edge we won't let the menu cross. Eight px
 *  matches the toolbar offsets in styles.css so the menu lines up with
 *  the rest of the chrome when it gets pushed back. */
const EDGE_PADDING = 8;

export function ContextMenu({ x, y, items, onClose }: ContextMenuProps) {
  const ref = useRef<HTMLDivElement | null>(null);
  // Adjusted position after we measure the menu and the parent host.
  // Seeded with the raw click coords so the menu paints at the right
  // spot on the *first* frame; the layout-effect below corrects on the
  // same frame if it would clip past an edge.
  const [pos, setPos] = useState({ left: x, top: y });

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

  // Clamp the menu inside the host's bounding rect once we know its
  // size. useLayoutEffect runs synchronously before paint, so we
  // never flash the menu in the clipped position before correcting.
  useLayoutEffect(() => {
    const el = ref.current;
    if (!el) return;
    const parent = el.parentElement;
    if (!parent) return;
    const hostRect = parent.getBoundingClientRect();
    const menuW = el.offsetWidth;
    const menuH = el.offsetHeight;
    const maxLeft = Math.max(EDGE_PADDING, hostRect.width - menuW - EDGE_PADDING);
    const maxTop = Math.max(EDGE_PADDING, hostRect.height - menuH - EDGE_PADDING);
    const clampedLeft = Math.min(Math.max(x, EDGE_PADDING), maxLeft);
    const clampedTop = Math.min(Math.max(y, EDGE_PADDING), maxTop);
    if (clampedLeft !== pos.left || clampedTop !== pos.top) {
      setPos({ left: clampedLeft, top: clampedTop });
    }
  }, [x, y, items, pos.left, pos.top]);

  return (
    <div
      ref={ref}
      className="lgc-menu"
      role="menu"
      style={{ left: pos.left, top: pos.top }}
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
