// The compile cache: compiled read root fields, reused by later requests
// whose compile would produce the same statements.
//
// An entry is keyed on what the compile actually read, not on the whole
// request: the variables the field's selection references, the claims
// the compile looked up (recorded in `CompileContext.claimReads`), the
// `$context` values it read, and whether the request is authenticated.
// A claim that a rule only substitutes into a `node` filter (`"$jwt.sub"`)
// is bound as a parameter: the entry keeps the statement text and swaps
// in each request's value, so per-user tokens share one compile.

import {
  Kind,
  type ASTNode,
  type FieldNode,
  type FragmentDefinitionNode,
} from "graphql";
import type { CompiledRead } from "./read.js";
import type { CompileContext } from "./context.js";
import { lookupPath } from "./auth.js";

/** Compiled root fields kept per field node, across keys. */
export const COMPILED_PER_FIELD = 16;
/** Compiled root fields kept in total, across field nodes. */
export const COMPILED_TOTAL = 4096;

/** A claim a rule binds as a filter value: rebound per request. */
interface Slot {
  statement: number;
  param: string;
  path: string;
}

interface Entry {
  variables: string;
  authenticated: boolean;
  statistics: number;
  /** Claims whose value shapes the statement: must be equal. */
  claims: Array<[path: string, key: string]>;
  /** Claims bound as parameters: any non-empty string, rebound. */
  slots: Slot[];
  /** Paths of `slots`, deduplicated. */
  slotPaths: string[];
  contextReads: Array<[string, unknown]>;
  compiled: CompiledRead;
}

type Compile = (jwt: Record<string, unknown> | undefined) => {
  ctx: CompileContext;
  compiled: CompiledRead;
};

export interface CacheRequest {
  fieldNodes: readonly FieldNode[];
  fragments: Record<string, FragmentDefinitionNode>;
  /** Coerced variable values, or undefined when there are none. */
  variables: Record<string, unknown> | undefined;
  jwt: Record<string, unknown> | undefined;
  context: unknown;
  statistics: number;
}

export class CompileCache {
  readonly #byField = new WeakMap<FieldNode, Entry[]>();
  /** Every live entry, oldest first, for the total cap. */
  readonly #live = new Map<Entry, FieldNode>();
  readonly #variableNames = new WeakMap<FieldNode, string[] | null>();
  /** Claims a field's compile bound into rule filters, to try as slots. */
  readonly #bindHints = new WeakMap<FieldNode, string[]>();
  readonly #total: number;

  constructor(total = COMPILED_TOTAL) {
    this.#total = total;
  }

  get size(): number {
    return this.#live.size;
  }

  /**
   * The cached compile for this request, or a fresh one from `compile`
   * (given a context with `jwt` as the claims), stored for later ones.
   */
  get(request: CacheRequest, compile: Compile): CompiledRead {
    const field = request.fieldNodes[0]!;
    const variables = this.#variablesKey(request);
    if (variables === undefined) return compile(request.jwt).compiled;
    const authenticated = !!request.jwt;
    const entries = this.#byField.get(field) ?? [];
    for (const e of entries) {
      if (
        e.variables !== variables ||
        e.authenticated !== authenticated ||
        e.statistics !== request.statistics ||
        !e.claims.every(
          ([path, key]) => stableKey(claimAt(request.jwt, path)) === key,
        ) ||
        !e.slotPaths.every((path) => bindable(claimAt(request.jwt, path))) ||
        !e.contextReads.every(
          ([path, value]) =>
            stableKey(lookupPath(request.context, path)) === stableKey(value),
        )
      ) {
        continue;
      }
      return e.slots.length === 0 ? e.compiled : rebind(e, request.jwt);
    }

    const miss = this.#compileMiss(field, request.jwt, compile);
    const { ctx, compiled, slots } = miss;
    const claims: Array<[string, string]> = [];
    for (const [path, read] of ctx.claimReads) {
      if (slots.some((s) => s.path === path)) continue;
      const key = stableKey(read.value);
      if (key === undefined) return compiled;
      claims.push([path, key]);
    }
    const entry: Entry = {
      variables,
      authenticated,
      statistics: request.statistics,
      claims,
      slots,
      slotPaths: [...new Set(slots.map((s) => s.path))],
      contextReads: ctx.contextReads,
      compiled,
    };
    entries.unshift(entry);
    this.#live.set(entry, field);
    for (const dropped of entries.splice(COMPILED_PER_FIELD)) {
      this.#live.delete(dropped);
    }
    this.#byField.set(field, entries);
    if (this.#live.size > this.#total) {
      const [oldest, owner] = this.#live.entries().next().value!;
      this.#live.delete(oldest);
      const list = this.#byField.get(owner);
      const at = list?.indexOf(oldest) ?? -1;
      if (at >= 0) list!.splice(at, 1);
    }
    return compiled;
  }

  /**
   * Compile on a miss. When an earlier compile of this field bound claims
   * into rule filters (the hint), compile with a marker in their place:
   * if each marker lands only as a whole parameter value, those
   * parameters are the claims' slots and the compile serves every value.
   * Otherwise compile with the real claims, and stop trying for the field.
   */
  #compileMiss(
    field: FieldNode,
    jwt: Record<string, unknown> | undefined,
    compile: Compile,
  ): { ctx: CompileContext; compiled: CompiledRead; slots: Slot[] } {
    const hint = this.#bindHints.get(field);
    if (jwt && hint && hint.length > 0) {
      const values = hint.map((path) => lookupPath(jwt, path));
      if (values.every(bindable)) {
        const markers = hint.map((_, i) => `${MARKER}${i}X`);
        let marked: unknown = jwt;
        hint.forEach((path, i) => {
          marked = withPath(marked, path.split("."), markers[i]);
        });
        let result: ReturnType<Compile> | undefined;
        try {
          result = compile(marked as Record<string, unknown>);
        } catch {
          result = undefined;
        }
        const slots = result && findSlots(result, hint, markers);
        if (result && slots) {
          const entry = { compiled: result.compiled, slots };
          return {
            ctx: result.ctx,
            compiled: rebind(entry, jwt),
            slots,
          };
        }
        this.#bindHints.set(field, []);
      }
    }
    const { ctx, compiled } = compile(jwt);
    // Learned from an authenticated compile: without claims, none are read.
    if (hint === undefined && jwt) {
      this.#bindHints.set(
        field,
        [...ctx.claimReads]
          .filter(([, read]) => read.bound && bindable(read.value))
          .map(([path]) => path),
      );
    }
    return { ctx, compiled, slots: [] };
  }

  /**
   * The variables the field's selection references, keyed; the whole
   * variable map when that cannot be told (fragment arguments).
   */
  #variablesKey(request: CacheRequest): string | undefined {
    const field = request.fieldNodes[0]!;
    let used = this.#variableNames.get(field);
    if (used === undefined) {
      used = variableNames(request.fieldNodes, request.fragments);
      this.#variableNames.set(field, used);
    }
    const values = request.variables;
    if (!values) return "";
    if (used === null) return stableKey(values);
    const picked: Record<string, unknown> = {};
    for (const name of used) {
      if (Object.hasOwn(values, name)) picked[name] = values[name];
    }
    return stableKey(picked);
  }
}

/** A claim value a cached compile may rebind: a non-empty string. */
function bindable(value: unknown): value is string {
  return typeof value === "string" && value.length > 0;
}

function claimAt(
  jwt: Record<string, unknown> | undefined,
  path: string,
): unknown {
  return path === "" ? jwt : lookupPath(jwt, path);
}

/** A marker no real claim holds; the uppercase X survives no lowercasing. */
const MARKER = "\u0000lora-claim-";

/**
 * The parameters holding each marker as their whole value. Undefined when
 * a marker is missing, appears anywhere else (statement text, inside a
 * value), or a hinted claim was not read as a bound value.
 */
function findSlots(
  result: { ctx: CompileContext; compiled: CompiledRead },
  paths: string[],
  markers: string[],
): Slot[] | undefined {
  for (const path of paths) {
    if (!result.ctx.claimReads.get(path)?.bound) return undefined;
  }
  const slots: Slot[] = [];
  const statements = result.compiled.statements;
  for (let s = 0; s < statements.length; s++) {
    const { text, params } = statements[s]!;
    if (text.includes(MARKER)) return undefined;
    for (const [param, value] of Object.entries(params)) {
      const i = markers.indexOf(value as string);
      if (i >= 0) slots.push({ statement: s, param, path: paths[i]! });
      else if (typeof value === "string" && value.includes(MARKER)) {
        return undefined;
      } else if (value !== null && typeof value === "object") {
        if ((stableKey(value) ?? MARKER).includes("lora-claim-")) {
          return undefined;
        }
      }
    }
  }
  if (!paths.every((path) => slots.some((s) => s.path === path))) {
    return undefined;
  }
  return slots;
}

/** A copy of `root` with `value` at `path`, sharing everything else. */
function withPath(root: unknown, path: string[], value: unknown): unknown {
  const [head, ...rest] = path;
  const copy: Record<string, unknown> = Array.isArray(root)
    ? ([...root] as unknown as Record<string, unknown>)
    : { ...(root as Record<string, unknown>) };
  copy[head!] =
    rest.length === 0
      ? value
      : withPath((root as Record<string, unknown>)[head!], rest, value);
  return copy;
}

/** The entry's compile with this request's claims in its slots. */
function rebind(
  entry: Pick<Entry, "compiled" | "slots">,
  jwt: Record<string, unknown> | undefined,
): CompiledRead {
  const statements = entry.compiled.statements.map((s) => ({
    text: s.text,
    params: s.params,
  }));
  const copied = new Set<number>();
  for (const slot of entry.slots) {
    const st = statements[slot.statement]!;
    if (!copied.has(slot.statement)) {
      st.params = { ...st.params };
      copied.add(slot.statement);
    }
    st.params[slot.param] = claimAt(jwt, slot.path);
  }
  return { ...entry.compiled, statements };
}

/**
 * The variables a field's selection references: in arguments, directives
 * and fragments it spreads. Null when a spread fragment declares its own
 * variables (fragment arguments), which this does not follow.
 */
function variableNames(
  fieldNodes: readonly FieldNode[],
  fragments: Record<string, FragmentDefinitionNode>,
): string[] | null {
  const names = new Set<string>();
  const visited = new Set<string>();
  let exact = true;
  const walk = (node: unknown): void => {
    if (Array.isArray(node)) {
      for (const n of node) walk(n);
      return;
    }
    if (node === null || typeof node !== "object") return;
    const ast = node as ASTNode;
    if (ast.kind === Kind.VARIABLE) {
      names.add(ast.name.value);
      return;
    }
    if (ast.kind === Kind.FRAGMENT_SPREAD) {
      const name = ast.name.value;
      if (!visited.has(name)) {
        visited.add(name);
        const fragment = fragments[name];
        if (fragment) {
          if (fragment.variableDefinitions?.length) exact = false;
          walk(fragment);
        }
      }
    }
    for (const [k, v] of Object.entries(ast)) {
      if (k === "loc") continue;
      if (v !== null && typeof v === "object") walk(v);
    }
  };
  walk(fieldNodes);
  return exact ? [...names].sort() : null;
}

/**
 * A deterministic key for JSON-like values (object keys sorted, bigints
 * tagged); undefined when the value cannot be keyed (functions, cycles,
 * class instances), which turns caching off for that request.
 */
export function stableKey(value: unknown): string | undefined {
  const seen = new Set<unknown>();
  let ok = true;
  const walk = (v: unknown): unknown => {
    if (typeof v === "bigint") return { $bigint: v.toString() };
    if (typeof v === "function" || typeof v === "symbol") {
      ok = false;
      return null;
    }
    if (v === null || typeof v !== "object") return v;
    if (seen.has(v)) {
      ok = false;
      return null;
    }
    seen.add(v);
    if (Array.isArray(v)) return v.map(walk);
    const proto = Object.getPrototypeOf(v);
    if (proto !== Object.prototype && proto !== null) {
      ok = false;
      return null;
    }
    const out: Record<string, unknown> = {};
    for (const k of Object.keys(v).sort()) {
      out[k] = walk((v as Record<string, unknown>)[k]);
    }
    return out;
  };
  const json = JSON.stringify(walk(value));
  return ok ? (json ?? "undefined") : undefined;
}
