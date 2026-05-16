/** Resolve an accessor against an object. Mirrors the kapsule's
 *  `accessor-fn` semantics: a function is invoked; a string is used as
 *  a property name; anything else (including undefined) is returned as
 *  is. */
export function readAccessor<T, In>(
  accessor: T | string | ((obj: In) => T) | undefined,
  obj: In,
): T | undefined {
  if (typeof accessor === "function") return (accessor as (o: In) => T)(obj);
  if (typeof accessor === "string") {
    return (obj as unknown as Record<string, unknown>)[accessor] as
      | T
      | undefined;
  }
  return accessor;
}

/** Resolve a node's display caption. Precedence:
 *    1. Host accessor (`nodeLabel`) when it returns a non-empty value.
 *       Strings pass through; HTMLElements yield their textContent;
 *       other primitives (e.g. numeric `nodeLabel="id"`) are stringified.
 *    2. The node's own `label` field, if it's a non-empty string.
 *    3. The node id, stringified.
 *  Returns "" when nothing resolves — callers should treat that as
 *  "skip drawing". */
export function resolveNodeLabelText<
  N extends { id?: string | number; label?: unknown },
>(
  accessor:
    | string
    | HTMLElement
    | ((n: N) => string | HTMLElement)
    | undefined,
  node: N,
): string {
  const raw = readAccessor<string | HTMLElement, N>(accessor, node);
  if (typeof raw === "string") return raw;
  if (typeof HTMLElement !== "undefined" && raw instanceof HTMLElement) {
    return raw.textContent ?? "";
  }
  if (raw !== null && raw !== undefined) return String(raw);
  if (typeof node.label === "string" && node.label.length > 0) return node.label;
  return node.id !== undefined ? String(node.id) : "";
}

/** Resolve a link's display caption. Same precedence as
 *  `resolveNodeLabelText`, with the id-based fallback rendering as
 *  `source → target` using the resolved endpoint ids. Returns "" when
 *  no caption can be formed. */
export function resolveLinkLabelText<
  L extends {
    label?: unknown;
    source: unknown;
    target: unknown;
  },
>(
  accessor:
    | string
    | HTMLElement
    | ((l: L) => string | HTMLElement)
    | undefined,
  link: L,
): string {
  const raw = readAccessor<string | HTMLElement, L>(accessor, link);
  if (typeof raw === "string") return raw;
  if (typeof HTMLElement !== "undefined" && raw instanceof HTMLElement) {
    return raw.textContent ?? "";
  }
  if (raw !== null && raw !== undefined) return String(raw);
  if (typeof link.label === "string" && link.label.length > 0) return link.label;
  const src = link.source as { id?: string | number } | string | number;
  const tgt = link.target as { id?: string | number } | string | number;
  const sId =
    typeof src === "object" && src !== null
      ? src.id
      : (src as string | number | undefined);
  const tId =
    typeof tgt === "object" && tgt !== null
      ? tgt.id
      : (tgt as string | number | undefined);
  if (sId === undefined && tId === undefined) return "";
  return `${sId ?? ""} → ${tId ?? ""}`;
}

/** Adjust a CSS color string toward the given alpha. Best-effort —
 *  passes through unrecognised inputs untouched. Used by the
 *  neighbour-highlight code to dim non-hovered neighbours. */
export function adjustAlpha(color: string, alpha: number): string {
  if (color.startsWith("#")) {
    const hex = color.slice(1);
    const full =
      hex.length === 3
        ? hex
            .split("")
            .map((c) => c + c)
            .join("")
        : hex;
    if (full.length === 6) {
      const r = parseInt(full.slice(0, 2), 16);
      const g = parseInt(full.slice(2, 4), 16);
      const b = parseInt(full.slice(4, 6), 16);
      return `rgba(${r}, ${g}, ${b}, ${alpha})`;
    }
  }
  const rgbMatch = color.match(/^rgb\(([^)]+)\)$/);
  if (rgbMatch) return `rgba(${rgbMatch[1]}, ${alpha})`;
  const rgbaMatch = color.match(/^rgba\(([^,]+),([^,]+),([^,]+),[^)]+\)$/);
  if (rgbaMatch)
    return `rgba(${rgbaMatch[1]},${rgbaMatch[2]},${rgbaMatch[3]},${alpha})`;
  return color;
}

/** Shared sentinel for the "no hover-highlight" state. Reusing one
 *  instance lets React's useState bail out when transitioning empty →
 *  empty (e.g. mouseleave → mouseleave), avoiding a no-op re-render plus
 *  downstream engineProps re-memo and kapsule re-bind. Treated as
 *  read-only — all consumers only call `.has()` on it. */
export const EMPTY_ID_SET: Set<string | number> = new Set<string | number>();
