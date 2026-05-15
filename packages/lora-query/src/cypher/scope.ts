import {
  StateEffect,
  StateField,
  type EditorState,
  type Extension,
} from "@codemirror/state";
import { EditorView, ViewPlugin, type ViewUpdate } from "@codemirror/view";
import { outline, type Outline, type OutlineVariable } from "../parser";

const EMPTY: Outline = {
  variables: [],
  parameters: [],
  labels: [],
  relTypes: [],
};

/**
 * Regex-based fallback outline. The WASM parser only succeeds on
 * complete queries; while the user is typing, the source rarely
 * parses. Rather than starve the completion popup of variable info,
 * we scan the source lexically for the structural cues that introduce
 * bindings — node / relationship patterns, `AS` aliases, `UNWIND`
 * variables, and `$param` references. Anything the WASM outline
 * already provides takes precedence.
 */
export function fallbackOutline(source: string): Outline {
  const variables: OutlineVariable[] = [];
  const seenVars = new Set<string>();
  const parameters: string[] = [];
  const seenParams = new Set<string>();
  const labels: string[] = [];
  const seenLabels = new Set<string>();
  const relTypes: string[] = [];
  const seenRelTypes = new Set<string>();

  const addVariable = (
    name: string,
    label: string | null,
    declStart: number,
    declEnd: number,
    kind: OutlineVariable["kind"],
    aliasOf: string | null,
  ) => {
    if (seenVars.has(name)) return;
    seenVars.add(name);
    variables.push({ name, declStart, declEnd, label, kind, aliasOf });
  };

  // Strip strings / comments so we don't pick up bindings from inside
  // them. Replace contents with spaces so byte offsets stay correct.
  const sanitised = stripStringsAndComments(source);

  // Node patterns: `(name)`, `(name:Label)`, `(name:Label { … })`.
  const nodeRe = /\(([A-Za-z_]\w*)(?::([A-Za-z_]\w*))?[^)]*\)/g;
  let m: RegExpExecArray | null;
  while ((m = nodeRe.exec(sanitised)) !== null) {
    const name = m[1]!;
    const label = m[2] ?? null;
    const start = m.index + 1;
    addVariable(name, label, start, start + name.length, "node", null);
    if (label && !seenLabels.has(label)) {
      seenLabels.add(label);
      labels.push(label);
    }
  }

  // Relationship patterns: `[r]`, `[r:TYPE]`, `[r:TYPE { … }]`.
  const relRe = /\[([A-Za-z_]\w*)(?::([A-Za-z_]\w*))?[^\]]*\]/g;
  while ((m = relRe.exec(sanitised)) !== null) {
    const name = m[1]!;
    const type = m[2] ?? null;
    const start = m.index + 1;
    addVariable(name, type, start, start + name.length, "relationship", null);
    if (type && !seenRelTypes.has(type)) {
      seenRelTypes.add(type);
      relTypes.push(type);
    }
  }

  // Bare label tokens that didn't appear in a node pattern — e.g.
  // `:Person` after a rel chain. Lower priority than the structural
  // pass above.
  const labelOnlyRe = /(?<!:):([A-Za-z_]\w*)/g;
  while ((m = labelOnlyRe.exec(sanitised)) !== null) {
    const name = m[1]!;
    if (!seenLabels.has(name) && !seenRelTypes.has(name)) {
      seenLabels.add(name);
      labels.push(name);
    }
  }

  // `<source> AS <alias>` — captures both the source (when it's a
  // simple identifier) and the alias so alias-property completion can
  // follow `WITH n AS x → x.| → n's label`.
  const asRe = /([A-Za-z_]\w*)\s+AS\s+([A-Za-z_]\w*)/gi;
  while ((m = asRe.exec(sanitised)) !== null) {
    const sourceName = m[1]!;
    const aliasName = m[2]!;
    const start = m.index + m[0].length - aliasName.length;
    // Pull the source's label so the alias inherits it for completion.
    const source = variables.find((v) => v.name === sourceName);
    addVariable(
      aliasName,
      source?.label ?? null,
      start,
      start + aliasName.length,
      "scalar",
      sourceName,
    );
  }

  // Bare `AS name` — used to catch aliases whose source is a complex
  // expression (e.g. `count(*) AS total`). The regex above won't match
  // because `count(*)` isn't a bare identifier.
  const bareAsRe = /\bAS\s+([A-Za-z_]\w*)/gi;
  while ((m = bareAsRe.exec(sanitised)) !== null) {
    const name = m[1]!;
    const start = m.index + m[0].length - name.length;
    if (!seenVars.has(name)) {
      addVariable(name, null, start, start + name.length, "scalar", null);
    }
  }

  // UNWIND <list> AS <name> — handled by the AS pass above.

  // $param references.
  const paramRe = /\$([A-Za-z_]\w*)/g;
  while ((m = paramRe.exec(sanitised)) !== null) {
    const name = m[1]!;
    if (!seenParams.has(name)) {
      seenParams.add(name);
      parameters.push(name);
    }
  }

  return { variables, parameters, labels, relTypes };
}

/** Replace string contents + comments with spaces, preserving offsets. */
function stripStringsAndComments(source: string): string {
  let i = 0;
  const out: string[] = [];
  let state: "normal" | "single" | "double" | "back" | "line" | "block" = "normal";
  while (i < source.length) {
    const c = source[i]!;
    if (state === "normal") {
      if (c === "'") {
        state = "single";
        out.push(c);
        i++;
        continue;
      }
      if (c === '"') {
        state = "double";
        out.push(c);
        i++;
        continue;
      }
      if (c === "`") {
        state = "back";
        out.push(c);
        i++;
        continue;
      }
      if (c === "/" && source[i + 1] === "/") {
        state = "line";
        out.push(" ", " ");
        i += 2;
        continue;
      }
      if (c === "/" && source[i + 1] === "*") {
        state = "block";
        out.push(" ", " ");
        i += 2;
        continue;
      }
      out.push(c);
      i++;
      continue;
    }
    if (state === "line") {
      if (c === "\n") {
        state = "normal";
        out.push(c);
      } else {
        out.push(" ");
      }
      i++;
      continue;
    }
    if (state === "block") {
      if (c === "*" && source[i + 1] === "/") {
        state = "normal";
        out.push(" ", " ");
        i += 2;
        continue;
      }
      out.push(c === "\n" ? "\n" : " ");
      i++;
      continue;
    }
    // inside a string
    if (c === "\\" && i + 1 < source.length) {
      out.push(" ", " ");
      i += 2;
      continue;
    }
    if ((state === "single" && c === "'") ||
        (state === "double" && c === '"') ||
        (state === "back" && c === "`")) {
      state = "normal";
      out.push(c);
    } else {
      out.push(c === "\n" ? "\n" : " ");
    }
    i++;
  }
  return out.join("");
}

/** Merge `b` into `a`, preferring entries already in `a`. */
function mergeOutlines(a: Outline, b: Outline): Outline {
  const names = new Set(a.variables.map((v) => v.name));
  const params = new Set(a.parameters);
  const labels = new Set(a.labels);
  const rels = new Set(a.relTypes);
  return {
    variables: [
      ...a.variables,
      ...b.variables.filter((v) => !names.has(v.name)),
    ],
    parameters: [
      ...a.parameters,
      ...b.parameters.filter((p) => !params.has(p)),
    ],
    labels: [...a.labels, ...b.labels.filter((l) => !labels.has(l))],
    relTypes: [...a.relTypes, ...b.relTypes.filter((t) => !rels.has(t))],
  };
}

/**
 * Effect that overwrites the outline StateField. Internal — only the
 * watcher should dispatch it in production. Exported so unit tests can
 * seed the state without mounting the editor.
 */
export const _setOutlineEffect = StateEffect.define<Outline>();
const setOutlineEffect = _setOutlineEffect;

/**
 * State field that holds the latest AST-derived outline (variables /
 * parameters / labels / rel-types). Updated asynchronously by the
 * watcher plugin below; completion sources read it via
 * {@link getOutline}.
 */
export const outlineField = StateField.define<Outline>({
  create: () => EMPTY,
  update(value, tr) {
    for (const effect of tr.effects) {
      if (effect.is(setOutlineEffect)) return effect.value;
    }
    return value;
  },
});

export function getOutline(state: EditorState): Outline {
  return state.field(outlineField, false) ?? EMPTY;
}

// Per-outline cached `name → variable` index. Built lazily on first
// lookup and stashed via a WeakMap keyed by the outline object itself
// so it gets garbage-collected with the outline. Hover, navigation and
// `resolveVariable` were each doing linear `.find()` scans on every
// invocation.
const variableIndexCache = new WeakMap<
  Outline,
  Map<string, OutlineVariable>
>();

function buildVariableIndex(outline: Outline): Map<string, OutlineVariable> {
  const map = new Map<string, OutlineVariable>();
  for (const v of outline.variables) {
    if (!map.has(v.name)) map.set(v.name, v);
  }
  return map;
}

/**
 * Constant-time variable lookup by name. Returns `null` when the
 * variable is not in scope. Use this instead of
 * `outline.variables.find(...)` — the index is cached per outline.
 */
export function findVariable(
  outline: Outline,
  name: string,
): OutlineVariable | null {
  let index = variableIndexCache.get(outline);
  if (!index) {
    index = buildVariableIndex(outline);
    variableIndexCache.set(outline, index);
  }
  return index.get(name) ?? null;
}

/**
 * ViewPlugin that debounces calls to the WASM `outline()` and pushes
 * results into {@link outlineField}. Failures are swallowed silently
 * — the previous outline simply stays in place.
 */
const outlineWatcher = ViewPlugin.fromClass(
  class {
    private pending: ReturnType<typeof setTimeout> | null = null;
    private generation = 0;

    constructor(view: EditorView) {
      this.schedule(view, 0);
    }

    update(update: ViewUpdate) {
      if (update.docChanged) this.schedule(update.view, 200);
    }

    private schedule(view: EditorView, delay: number) {
      if (this.pending) clearTimeout(this.pending);
      const gen = ++this.generation;
      this.pending = setTimeout(() => {
        this.pending = null;
        const source = view.state.doc.toString();
        if (!source) {
          view.dispatch({ effects: setOutlineEffect.of(EMPTY) });
          return;
        }
        // Always seed with the regex fallback so the popup has
        // *something* to show while the query is mid-typing and the
        // WASM parse fails. Once the parse succeeds, the WASM result
        // wins (with `label` info from the AST that the regex pass
        // can't always recover, like comma-separated label sets).
        const seed = fallbackOutline(source);
        view.dispatch({ effects: setOutlineEffect.of(seed) });
        outline(source)
          .then((next) => {
            if (gen !== this.generation) return;
            view.dispatch({
              effects: setOutlineEffect.of(mergeOutlines(next, seed)),
            });
          })
          .catch(() => {});
      }, delay);
    }

    destroy() {
      if (this.pending) clearTimeout(this.pending);
    }
  },
);

export const outlineExtension: Extension = [outlineField, outlineWatcher];
