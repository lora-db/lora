import { Facet, type EditorState } from "@codemirror/state";

/** Context passed to the property-key callback. */
export interface PropertyContext {
  /** "node" when the cursor is inside `( ... { ... } ... )`, "relationship" inside `[ ... { ... } ... ]`, "map" otherwise. */
  kind: "node" | "relationship" | "map";
  /** First label / rel-type seen on the surrounding pattern (best-effort). */
  label: string | null;
  /** Variable bound on the surrounding pattern, if any. */
  variable: string | null;
}

export interface ProcedureSignature {
  /** Fully qualified procedure name, e.g. `db.indexes`. */
  name: string;
  /** One-line signature, e.g. `db.indexes() :: (name, state, type)`. */
  signature?: string;
  /** Optional doc string. */
  info?: string;
}

export interface LoraQueryProviders {
  /** Known node labels to suggest after `:` in a node pattern. */
  labels: readonly string[];
  /** Known relationship types to suggest after `:` in a relationship pattern. */
  relTypes: readonly string[];
  /** Known stored procedures — suggested after `CALL `/`YIELD `. */
  procedures: readonly ProcedureSignature[];
  /**
   * Called when the cursor is inside a `{ ... }` property map. The host
   * decides what to return based on the surrounding label/variable.
   * The result may be synchronous or a Promise — the completion popup
   * waits for the latter.
   */
  getPropertyKeys?: (
    ctx: PropertyContext,
    state: EditorState,
  ) => readonly string[] | Promise<readonly string[]>;
}

const EMPTY: LoraQueryProviders = { labels: [], relTypes: [], procedures: [] };

/**
 * Per-editor configuration for the completion popup. Hosts set this via
 * the `LoraQueryEditor` props (`labels`, `relTypes`, `getPropertyKeys`)
 * which the editor wires into a Compartment-managed facet.
 */
export const loraQueryProviders = Facet.define<
  LoraQueryProviders,
  LoraQueryProviders
>({
  combine: (values) => values[values.length - 1] ?? EMPTY,
});

export function getProviders(state: EditorState): LoraQueryProviders {
  return state.facet(loraQueryProviders);
}
