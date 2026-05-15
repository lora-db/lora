import { hoverTooltip, type Tooltip } from "@codemirror/view";
import { findToken } from "./data";
import { findVariable, getOutline } from "./scope";

/**
 * Hover tooltip combining two sources of truth:
 *  - the static Cypher token table (keywords, functions, namespaces),
 *  - the live outline of declared variables (where they were bound,
 *    inferred label).
 */
export const cypherHover = hoverTooltip(
  (view, pos, side): Tooltip | null => {
    const line = view.state.doc.lineAt(pos);
    const { from, text } = line;
    const offset = pos - from;
    if (offset < 0 || offset > text.length) return null;

    let start = offset;
    let end = offset;
    while (start > 0 && /[\w.]/.test(text[start - 1] ?? "")) start--;
    while (end < text.length && /[\w.]/.test(text[end] ?? "")) end++;
    if (start === end) return null;
    if (offset === start && side < 0) return null;
    if (offset === end && side > 0) return null;

    const word = text.slice(start, end);

    // 1. Look up in the outline first — variables shadow keywords.
    const outline = getOutline(view.state);
    const v = findVariable(outline, word);
    if (v) {
      return makeTooltip(from + start, from + end, () => {
        const dom = document.createElement("div");
        dom.className = "cm-tooltip-lora-query";
        const title = document.createElement("div");
        title.className = "cm-tooltip-lora-query__title";
        const strong = document.createElement("strong");
        strong.textContent = v.name;
        title.appendChild(strong);
        if (v.label) {
          const code = document.createElement("code");
          code.textContent = `:${v.label}`;
          title.appendChild(document.createTextNode(" "));
          title.appendChild(code);
        }
        const body = document.createElement("div");
        body.className = "cm-tooltip-lora-query__body";
        const declLine =
          view.state.doc.lineAt(v.declStart).number;
        body.textContent = v.label
          ? `Variable bound on line ${declLine} (label \`${v.label}\`). ⌘-click or F12 to jump.`
          : `Variable bound on line ${declLine}. ⌘-click or F12 to jump.`;
        dom.appendChild(title);
        dom.appendChild(body);
        return { dom };
      });
    }

    // 2. Static token (keyword, function, namespace).
    const token = findToken(word);
    if (!token) return null;
    return makeTooltip(from + start, from + end, () => {
      const dom = document.createElement("div");
      dom.className = "cm-tooltip-lora-query";
      const title = document.createElement("div");
      title.className = "cm-tooltip-lora-query__title";
      const strong = document.createElement("strong");
      strong.textContent = token.label;
      title.appendChild(strong);
      if (token.detail) {
        const code = document.createElement("code");
        code.textContent = token.detail;
        title.appendChild(document.createTextNode(" "));
        title.appendChild(code);
      }
      const body = document.createElement("div");
      body.className = "cm-tooltip-lora-query__body";
      body.textContent = token.info;
      dom.appendChild(title);
      dom.appendChild(body);
      return { dom };
    });
  },
  { hoverTime: 200 },
);

function makeTooltip(
  pos: number,
  end: number,
  create: Tooltip["create"],
): Tooltip {
  return {
    pos,
    end,
    above: true,
    create,
  };
}
