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
//
// The same is done for a variable the caller names as rebindable: the key
// of a lookup by @key. `festival(key: $key)` compiles to the same
// statement for every key, with the key as a parameter, so the entry is
// keyed on everything but that variable and its value is swapped in.
// Only when the key is the variable's one use in the field: anywhere else
// the compile may check the value (a filter's string limit), and a value
// swapped in afterwards is one it never saw.

import {
  GraphQLError,
  Kind,
  type ASTNode,
  type FieldNode,
  type FragmentDefinitionNode,
} from "graphql";
import type { CompiledRead } from "./read/types.js";
import type { CompileContext } from "./context.js";
import { lookupPath } from "./auth.js";

/** Compiled root fields kept per field node, across keys. */
export const COMPILED_PER_FIELD = 16;
/** Compiled root fields kept in total, across field nodes. */
export const COMPILED_TOTAL = 4096;
/** Default `compileCacheBytes`: the approximate size the cache may reach. */
export const COMPILE_CACHE_BYTES = 64 * 1024 * 1024;
/**
 * Requests whose variables key is longer than this are compiled but not
 * cached: a 10 000-item `in:` list or an embedding vector rarely repeats,
 * and would hold its statement's parameters for nothing.
 */
export const MAX_CACHED_VARIABLES = 16 * 1024;
/** What an entry costs beyond its strings: the shape closure, maps. */
const ENTRY_OVERHEAD = 1024;

/** A claim a rule binds as a filter value: rebound per request. */
interface Slot {
  statement: number;
  param: string;
  path: string;
}

/** A variable bound as a whole parameter value: rebound per request. */
interface VariableSlot {
  statement: number;
  param: string;
  name: string;
}

interface Entry {
  /** The variables the compile depends on, without the rebound ones. */
  variables: string;
  /** Variables bound as parameters, and their names joined (the key). */
  variableSlots: VariableSlot[];
  rebound: string;
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
  /** Approximate bytes the entry holds. */
  bytes: number;
}

/** A compile the cache did not have, and what an entry keeps of it. */
interface Miss {
  ctx: CompileContext;
  compiled: CompiledRead;
  template: CompiledRead;
  slots: Slot[];
  variableSlots: VariableSlot[];
}

type Compile = (
  jwt: Record<string, unknown> | undefined,
  /** Values to compile with in place of the request's variables. */
  variables?: Record<string, unknown>,
) => {
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
  /**
   * Variables whose value only ever lands in the statement as a whole
   * parameter: the variable given as the key of a lookup by @key. One
   * compile then serves every value of them.
   */
  rebindable?: readonly string[] | undefined;
}

export class CompileCache {
  readonly #byField = new WeakMap<FieldNode, Entry[]>();
  /**
   * Every live entry, oldest first, with its field's entry list, for the
   * caps. The list, not the field node: a field node's location pins its
   * whole document (source and tokens), which must be free to go once
   * the document cache drops it.
   */
  readonly #live = new Map<Entry, Entry[]>();
  readonly #variableNames = new WeakMap<FieldNode, VariableUses | null>();
  /** Claims a field's compile bound into rule filters, to try as slots. */
  readonly #bindHints = new WeakMap<FieldNode, string[]>();
  /** Fields whose rebindable variable did not land as a whole parameter. */
  readonly #notRebindable = new WeakSet<FieldNode>();
  /** Drops the entries of a field node that was garbage collected. */
  readonly #collected =
    typeof FinalizationRegistry === "function"
      ? new FinalizationRegistry<Entry[]>((entries) => {
          for (const e of entries.splice(0)) this.#drop(e);
        })
      : undefined;
  readonly #total: number;
  readonly #maxBytes: number;
  #bytes = 0;

  constructor(total = COMPILED_TOTAL, maxBytes = COMPILE_CACHE_BYTES) {
    this.#total = total;
    this.#maxBytes = maxBytes;
  }

  get size(): number {
    return this.#live.size;
  }

  /** Approximate bytes the cached entries hold. */
  get bytes(): number {
    return this.#bytes;
  }

  #drop(entry: Entry): void {
    if (this.#live.delete(entry)) this.#bytes -= entry.bytes;
  }

  /**
   * The cached compile for this request, or a fresh one from `compile`
   * (given a context with `jwt` as the claims), stored for later ones.
   */
  get(request: CacheRequest, compile: Compile): CompiledRead {
    const field = request.fieldNodes[0]!;
    // The variables to rebind: only when each has a value a parameter can
    // simply take and no other use in the field, and the field has not
    // shown that it cannot be done.
    const rebindable = request.rebindable ?? [];
    const once = this.#uses(request)?.once;
    const names =
      once &&
      !this.#notRebindable.has(field) &&
      rebindable.every(
        (name) => once.has(name) && rebindableValue(request.variables?.[name]),
      )
        ? rebindable
        : [];
    const variables = this.#variablesKey(request, names);
    if (variables === undefined || variables.length > MAX_CACHED_VARIABLES) {
      return compile(request.jwt).compiled;
    }
    const authenticated = !!request.jwt;
    const reboundKey = names.join("\0");
    const entries = this.#byField.get(field) ?? [];
    for (const e of entries) {
      if (
        e.variables !== variables ||
        e.rebound !== reboundKey ||
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
      return e.slots.length === 0 && e.variableSlots.length === 0
        ? e.compiled
        : rebind(e, request.jwt, request.variables);
    }

    const miss = this.#compileMiss(field, request, names, compile);
    const { ctx, compiled, slots, variableSlots } = miss;
    // The rebindable variables did not all become slots: the entry would
    // be keyed without a value its statement depends on. Compiled, not
    // cached; the field is remembered and its next request keys on them.
    if (names.length > 0 && variableSlots.length === 0) return compiled;
    const claims: Array<[string, string]> = [];
    for (const [path, read] of ctx.claimReads) {
      if (slots.some((s) => s.path === path)) continue;
      const key = stableKey(read.value);
      if (key === undefined) return compiled;
      claims.push([path, key]);
    }
    const entry: Entry = {
      variables,
      variableSlots,
      rebound: reboundKey,
      authenticated,
      statistics: request.statistics,
      claims,
      slots,
      slotPaths: [...new Set(slots.map((s) => s.path))],
      contextReads: ctx.contextReads,
      compiled: miss.template,
      bytes: entryBytes(variables, claims, compiled),
    };
    if (entry.bytes > this.#maxBytes) return compiled;
    if (!this.#byField.has(field)) {
      this.#byField.set(field, entries);
      this.#collected?.register(field, entries);
    }
    entries.unshift(entry);
    this.#live.set(entry, entries);
    this.#bytes += entry.bytes;
    for (const dropped of entries.splice(COMPILED_PER_FIELD)) {
      this.#drop(dropped);
    }
    while (this.#live.size > this.#total || this.#bytes > this.#maxBytes) {
      const [oldest, list] = this.#live.entries().next().value!;
      this.#drop(oldest);
      const at = list.indexOf(oldest);
      if (at >= 0) list.splice(at, 1);
    }
    return compiled;
  }

  /**
   * Compile on a miss. When an earlier compile of this field bound claims
   * into rule filters (the hint), compile with a marker in their place:
   * if each marker lands only as a whole parameter value, those
   * parameters are the claims' slots and the compile serves every value.
   * The same for the variables to rebind. Otherwise compile with the real
   * values, and stop trying for the field.
   *
   * `template` is what the entry keeps (markers in its slots); `compiled`
   * is the same with this request's values.
   */
  #compileMiss(
    field: FieldNode,
    request: CacheRequest,
    names: readonly string[],
    compile: Compile,
  ): Miss {
    const jwt = request.jwt;
    // With the request's own values: nothing to rebind.
    const plain = (): Miss => {
      const { ctx, compiled } = compile(jwt);
      return {
        ctx,
        compiled,
        template: compiled,
        slots: [],
        variableSlots: [],
      };
    };
    const hint = this.#bindHints.get(field);
    const hinted =
      jwt && hint && hint.length > 0
        ? hint.every((path) => bindable(lookupPath(jwt, path)))
        : false;
    if (hinted || names.length > 0) {
      const claimMarkers = hinted ? hint!.map((_, i) => `${MARKER}${i}X`) : [];
      let marked: unknown = jwt;
      if (hinted) {
        hint!.forEach((path, i) => {
          marked = withPath(marked, path.split("."), claimMarkers[i]);
        });
      }
      // A marker of the value's own type, so a typed argument takes it.
      const variableMarkers = names.map((name, i) =>
        typeof request.variables?.[name] === "number"
          ? NUMBER_MARKER - i
          : `${VARIABLE_MARKER}${i}X`,
      );
      let result: ReturnType<Compile> | undefined;
      try {
        result = compile(
          marked as Record<string, unknown> | undefined,
          names.length > 0
            ? Object.fromEntries(names.map((n, i) => [n, variableMarkers[i]]))
            : undefined,
        );
      } catch (error) {
        // A compile may refuse a marker as it would a bad value; anything
        // else is not the marker's doing.
        if (!(error instanceof GraphQLError)) throw error;
        result = undefined;
      }
      if (!result) {
        // Whether the markers were refused or the request itself: when the
        // real values do not compile either (an unauthenticated request),
        // the error is the request's and the field is tried again.
        const real = plain();
        if (hinted) this.#bindHints.set(field, []);
        if (names.length > 0) this.#notRebindable.add(field);
        return real;
      }
      const slots = hinted ? findSlots(result, hint!, claimMarkers) : [];
      const variableSlots =
        names.length > 0
          ? findVariableSlots(result, names, variableMarkers)
          : [];
      if (slots && variableSlots) {
        const entry = { compiled: result.compiled, slots, variableSlots };
        return {
          ctx: result.ctx,
          template: result.compiled,
          compiled: rebind(entry, jwt, request.variables),
          slots,
          variableSlots,
        };
      }
      // Whichever did not work out is not tried again for this field.
      if (hinted && !slots) this.#bindHints.set(field, []);
      if (names.length > 0 && !variableSlots) {
        this.#notRebindable.add(field);
        return plain();
      }
      return this.#compileMiss(field, request, names, compile);
    }
    const real = plain();
    const { ctx } = real;
    // Learned from an authenticated compile: without claims, none are read.
    if (hint === undefined && jwt) {
      this.#bindHints.set(
        field,
        [...ctx.claimReads]
          .filter(([, read]) => read.bound && bindable(read.value))
          .map(([path]) => path),
      );
    }
    return real;
  }

  /**
   * The variables the field's selection references, keyed; the whole
   * variable map when that cannot be told (fragment arguments).
   */
  #variablesKey(
    request: CacheRequest,
    rebound: readonly string[],
  ): string | undefined {
    const used = this.#uses(request);
    const values = request.variables;
    if (!values) return "";
    if (used === null) return stableKey(values);
    const picked: Record<string, unknown> = {};
    for (const name of used.names) {
      if (rebound.includes(name)) continue;
      if (Object.hasOwn(values, name)) picked[name] = values[name];
    }
    return stableKey(picked);
  }

  #uses(request: CacheRequest): VariableUses | null {
    const field = request.fieldNodes[0]!;
    let used = this.#variableNames.get(field);
    if (used === undefined) {
      used = variableNames(request.fieldNodes, request.fragments);
      this.#variableNames.set(field, used);
    }
    return used;
  }
}

/**
 * An entry's approximate size: its strings at two bytes a character, the
 * parameters as large as the variables they mostly come from, and a fixed
 * overhead.
 */
function entryBytes(
  variables: string,
  claims: Array<[string, string]>,
  compiled: CompiledRead,
): number {
  let chars = 2 * variables.length;
  for (const [path, key] of claims) chars += path.length + key.length;
  for (const s of compiled.statements) chars += s.text.length;
  return 2 * chars + ENTRY_OVERHEAD;
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
/** The same for a rebound variable, as a string and as a number. */
const VARIABLE_MARKER = "\u0000lora-variable-";
/** As it reads inside a `stableKey()`. */
const VARIABLE_MARKER_KEYED = JSON.stringify(VARIABLE_MARKER).slice(1, -1);
const NUMBER_MARKER = Number.MIN_SAFE_INTEGER + 1024;

/**
 * A variable value a cached compile may rebind: a string or a whole
 * number, the kinds a @key holds. `null`, a list or an object would
 * compile to a different statement.
 */
function rebindableValue(value: unknown): boolean {
  return (
    (typeof value === "string" &&
      value.length > 0 &&
      !value.includes("\u0000")) ||
    (typeof value === "number" &&
      Number.isSafeInteger(value) &&
      value > NUMBER_MARKER)
  );
}

/**
 * The parameters holding each variable's marker as their whole value.
 * Undefined when a marker is missing or appears anywhere else: in the
 * statement text, inside a list or a map, or changed on the way (a value
 * the compile converts or checks is not one it merely passes on).
 */
function findVariableSlots(
  result: { compiled: CompiledRead },
  names: readonly string[],
  markers: ReadonlyArray<string | number>,
): VariableSlot[] | undefined {
  const slots: VariableSlot[] = [];
  const statements = result.compiled.statements;
  for (let s = 0; s < statements.length; s++) {
    const { text, params } = statements[s]!;
    if (
      text.includes(VARIABLE_MARKER) ||
      markers.some((m) => typeof m === "number" && text.includes(String(m)))
    ) {
      return undefined;
    }
    for (const [param, value] of Object.entries(params)) {
      const i = markers.indexOf(value as string | number);
      if (i >= 0) {
        slots.push({ statement: s, param, name: names[i]! });
      } else if (value !== null && typeof value === "object") {
        const inside = stableKey(value) ?? VARIABLE_MARKER_KEYED;
        if (
          inside.includes(VARIABLE_MARKER_KEYED) ||
          markers.some(
            (m) => typeof m === "number" && inside.includes(String(m)),
          )
        ) {
          return undefined;
        }
      } else if (typeof value === "string" && value.includes(VARIABLE_MARKER)) {
        return undefined;
      }
    }
  }
  return names.every((name) => slots.some((slot) => slot.name === name))
    ? slots
    : undefined;
}

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

/** The entry's compile with this request's claims and variables in its slots. */
function rebind(
  entry: Pick<Entry, "compiled" | "slots" | "variableSlots">,
  jwt: Record<string, unknown> | undefined,
  variables: Record<string, unknown> | undefined,
): CompiledRead {
  const statements = entry.compiled.statements.map((s) => ({
    text: s.text,
    params: s.params,
  }));
  const copied = new Set<number>();
  const set = (statement: number, param: string, value: unknown) => {
    const st = statements[statement]!;
    if (!copied.has(statement)) {
      st.params = { ...st.params };
      copied.add(statement);
    }
    st.params[param] = value;
  };
  for (const slot of entry.slots) {
    set(slot.statement, slot.param, claimAt(jwt, slot.path));
  }
  for (const slot of entry.variableSlots) {
    set(slot.statement, slot.param, variables?.[slot.name]);
  }
  return { ...entry.compiled, statements };
}

interface VariableUses {
  /** Sorted. */
  names: string[];
  /** Those the selection references exactly once. */
  once: Set<string>;
}

/**
 * The variables a field's selection references: in arguments, directives
 * and fragments it spreads. Null when a spread fragment declares its own
 * variables (fragment arguments), which this does not follow.
 */
function variableNames(
  fieldNodes: readonly FieldNode[],
  fragments: Record<string, FragmentDefinitionNode>,
): VariableUses | null {
  const names = new Map<string, number>();
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
      names.set(ast.name.value, (names.get(ast.name.value) ?? 0) + 1);
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
  if (!exact) return null;
  return {
    names: [...names.keys()].sort(),
    once: new Set([...names].filter(([, n]) => n === 1).map(([name]) => name)),
  };
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
