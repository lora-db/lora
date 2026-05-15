import { EditorState, StateField, type Extension } from "@codemirror/state";
import { EditorView, showTooltip, type Tooltip } from "@codemirror/view";
import {
  CYPHER_TOP_LEVEL_FUNCTIONS,
  NAMESPACE_MEMBERS,
  type CypherToken,
} from "./data";

/**
 * Live signature hint shown while the cursor is inside a function
 * call's argument list (after `name(`). We resolve the function name
 * from the metadata tables — namespaced (`math.abs`) and top-level
 * (`count`) — and render a small tooltip at the call site.
 */
function findSignatureContext(state: EditorState): {
  pos: number;
  token: CypherToken;
} | null {
  const cursor = state.selection.main.head;
  const text = state.doc.toString();
  const head = text.slice(0, cursor);

  // Walk back tracking nesting so we land on the closest unclosed `(`.
  let depth = 0;
  let openParen = -1;
  let str: "'" | '"' | "`" | null = null;
  for (let i = head.length - 1; i >= 0; i--) {
    const c = head[i]!;
    if (str) {
      if (c === "\\") {
        i--;
        continue;
      }
      if (c === str) str = null;
      continue;
    }
    if (c === "'" || c === '"' || c === "`") {
      str = c as "'" | '"' | "`";
      continue;
    }
    if (c === ")") depth++;
    else if (c === "(") {
      if (depth === 0) {
        openParen = i;
        break;
      }
      depth--;
    }
  }
  if (openParen < 0) return null;

  // Match the identifier (with optional dotted namespace) directly
  // preceding the `(`.
  const beforeParen = head.slice(0, openParen);
  const match = /([A-Za-z_][\w]*(?:\.[A-Za-z_][\w]*)*)\s*$/.exec(beforeParen);
  if (!match) return null;
  const name = match[1]!;

  const token = resolveFunction(name);
  if (!token) return null;
  return { pos: openParen + 1, token };
}

function resolveFunction(name: string): CypherToken | null {
  if (name.includes(".")) {
    const [ns, member] = name.split(".") as [string, string];
    const members = NAMESPACE_MEMBERS[ns.toLowerCase()];
    if (!members) return null;
    return members.find((m) => m.label === member) ?? null;
  }
  return (
    CYPHER_TOP_LEVEL_FUNCTIONS.find(
      (f) => f.label.toLowerCase() === name.toLowerCase(),
    ) ?? null
  );
}

function buildTooltip(token: CypherToken, pos: number): Tooltip {
  return {
    pos,
    above: true,
    arrow: true,
    create() {
      const dom = document.createElement("div");
      dom.className = "cm-tooltip-lora-query cm-tooltip-lora-signature";
      const head = document.createElement("div");
      head.className = "cm-tooltip-lora-query__title";
      const code = document.createElement("code");
      code.textContent = token.detail ?? `${token.label}(...)`;
      head.appendChild(code);
      dom.appendChild(head);
      if (token.info) {
        const body = document.createElement("div");
        body.className = "cm-tooltip-lora-query__body";
        body.textContent = token.info;
        dom.appendChild(body);
      }
      return { dom };
    },
  };
}

const signatureField = StateField.define<readonly Tooltip[]>({
  create: (state) => {
    const ctx = findSignatureContext(state);
    return ctx ? [buildTooltip(ctx.token, ctx.pos)] : [];
  },
  update(value, tr) {
    if (!tr.docChanged && tr.selection === undefined) return value;
    const ctx = findSignatureContext(tr.state);
    return ctx ? [buildTooltip(ctx.token, ctx.pos)] : [];
  },
  provide: (f) => showTooltip.computeN([f], (state) => state.field(f)),
});

/** Wire signature hints into the editor. */
export const signatureHint: Extension = [
  signatureField,
  EditorView.theme({
    ".cm-tooltip-lora-signature": {
      maxWidth: "420px",
    },
  }),
];
