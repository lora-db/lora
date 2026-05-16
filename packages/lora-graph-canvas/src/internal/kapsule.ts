// Minimal kapsule DSL — a chainable, prop-driven component factory.
//
// LORA: internalised from `kapsule` (MIT, © Vasco Asturiano). Ported
// to TS with proper types; the runtime is preserved verbatim except
// we depend on our own tiny `debounce` (../internal/debounce.ts)
// instead of `lodash-es/debounce.js`.
//
// Shape: `Kapsule(cfg)` returns a constructor. Instances expose every
// prop name as a chainable getter/setter and every method name as a
// pass-through. Internally, prop changes mark the instance dirty and
// fire `update(state, changedProps)` on the trailing edge of a 1ms
// debounce; the `state` object is built from `stateInit()` at
// construction and persists for the lifetime of the instance.

import { debounce } from "./debounce";

export interface KapsuleState {
  initialised: boolean;
  _rerender: () => void;
  [key: string]: unknown;
}

export interface PropConfig {
  /** Default prop value, applied at construction time. */
  default?: unknown;
  /** Fire `update()` after this prop changes. Default true. */
  triggerUpdate?: boolean;
  /** Callback fired synchronously when the prop changes. */
  onChange?: (newVal: unknown, state: KapsuleState, prevVal: unknown) => void;
}

// LORA: `...args: never[]` is the bivariant escape hatch — under
// strict variance any concrete-args function (e.g. `(state, x?: number,
// y?: number) => ...`) is assignable to this shape. That keeps the
// downstream `methods: { ... }` table strongly typed at the consumer
// while accepting it through the loose KapsuleConfig surface here.
export type KapsuleMethod = (
  state: KapsuleState,
  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  ...args: any[]
) => unknown;

export interface KapsuleConfig {
  props?: Record<string, PropConfig>;
  methods?: Record<string, KapsuleMethod>;
  /** Alias one prop/method name to another (for backwards compat). */
  aliases?: Record<string, string>;
  /** Initial state factory. Run once at construction. */
  stateInit?: (options?: Record<string, unknown>) => Partial<KapsuleState>;
  /** Constructor-time initialiser. Receives the host element (or
   *  whatever was passed to `new Kapsule(...)`). Typed `any` so
   *  consumers can declare a narrower DOM type without an extra cast. */
  init?: (
    // eslint-disable-next-line @typescript-eslint/no-explicit-any
    constructorItem: any,
    state: KapsuleState,
    options?: Record<string, unknown>,
  ) => void;
  /** Trailing-edge digest. Fired after each batch of prop changes. */
  update?: (
    state: KapsuleState,
    changedProps: Record<string, unknown>,
  ) => void;
}

/** A kapsule instance — every prop/method ends up as a chainable
 *  property on the result. We type it loosely (any-keyed) because
 *  the concrete shape depends on the caller's `props`/`methods`;
 *  consumers can narrow by declaring their own interface and casting
 *  the return through `unknown`. */
export interface KapsuleInstance {
  (constructorItem?: unknown): KapsuleInstance;
  resetProps: () => KapsuleInstance;
  [propOrMethod: string]: (...args: unknown[]) => unknown;
}

export interface KapsuleClassCtor {
  new (
    element?: unknown,
    options?: Record<string, unknown>,
  ): KapsuleInstance;
  (
    element?: unknown,
    options?: Record<string, unknown>,
  ): KapsuleInstance;
}

class Prop {
  readonly name: string;
  readonly defaultVal: unknown;
  readonly triggerUpdate: boolean;
  readonly onChange: (
    newVal: unknown,
    state: KapsuleState,
    prevVal: unknown,
  ) => void;

  constructor(name: string, cfg: PropConfig) {
    this.name = name;
    this.defaultVal = cfg.default ?? null;
    this.triggerUpdate = cfg.triggerUpdate ?? true;
    this.onChange = cfg.onChange ?? (() => {});
  }
}

export default function Kapsule(cfg: KapsuleConfig = {}): KapsuleClassCtor {
  const {
    stateInit = () => ({}),
    props: rawProps = {},
    methods = {},
    aliases = {},
    init: initFn = () => {},
    update: updateFn = () => {},
  } = cfg;

  const props = Object.keys(rawProps).map(
    (name) => new Prop(name, rawProps[name]!),
  );

  function KapsuleComp(this: unknown, ...rawArgs: unknown[]): KapsuleInstance {
    // Dual-call form: `new K(el)` is the class-mode flow; `K()` is
    // the bare-factory flow. In class mode we eat the first arg as
    // the host element.
    const classMode = this instanceof KapsuleComp;
    const args = rawArgs.slice();
    const nodeElement = classMode ? args.shift() : undefined;
    const options = (args[0] as Record<string, unknown>) ?? {};

    const initial =
      typeof stateInit === "function" ? stateInit(options) : stateInit;
    const state: KapsuleState = Object.assign(
      {},
      initial,
      { initialised: false, _rerender: () => {} } as KapsuleState,
    );

    let changedProps: Record<string, unknown> = {};

    const comp = ((el?: unknown) => {
      initStatic(el, options);
      digest();
      return comp;
    }) as unknown as KapsuleInstance;

    function initStatic(
      el: unknown,
      opts: Record<string, unknown>,
    ): void {
      initFn.call(comp, el, state, opts);
      state.initialised = true;
    }

    const digest = debounce(() => {
      if (!state.initialised) return;
      updateFn.call(comp, state, changedProps);
      changedProps = {};
    }, 1);

    // Build chainable getter/setter for each prop.
    for (const prop of props) {
      const { name, triggerUpdate: redigest, onChange, defaultVal } = prop;
      const fn = (val?: unknown): unknown => {
        const curVal = state[name];
        if (arguments.length === 0) return curVal; // getter
        // Argument explicitly passed — handle undefined → default.
        const next = val === undefined ? defaultVal : val;
        state[name] = next;
        onChange.call(comp, next, state, curVal);
        if (!Object.prototype.hasOwnProperty.call(changedProps, name)) {
          changedProps[name] = curVal;
        }
        if (redigest) digest();
        return comp;
      };
      // Re-wire to forward arguments faithfully (forEach loses
      // `arguments.length === 0` detection if we relied on the val
      // default above). Use a real function expression instead.
      comp[name] = function setterGetter(): unknown {
        const curVal = state[name];
        // eslint-disable-next-line prefer-rest-params
        if (arguments.length === 0) return curVal;
        // eslint-disable-next-line prefer-rest-params
        const arg = arguments[0];
        const next = arg === undefined ? defaultVal : arg;
        state[name] = next;
        onChange.call(comp, next, state, curVal);
        if (!Object.prototype.hasOwnProperty.call(changedProps, name)) {
          changedProps[name] = curVal;
        }
        if (redigest) digest();
        return comp;
      };
      // `fn` retained but unused — left out of comp to avoid a
      // second copy under the same name.
      void fn;
    }

    // Methods: pass-through, with `comp` as `this` and `state` as
    // the first arg.
    for (const methodName of Object.keys(methods)) {
      comp[methodName] = function (...mArgs: unknown[]) {
        return methods[methodName]!.call(comp, state, ...mArgs);
      };
    }

    // Aliases: another name for the same callable.
    for (const [alias, target] of Object.entries(aliases)) {
      comp[alias] = comp[target] as (...args: unknown[]) => unknown;
    }

    comp.resetProps = function () {
      for (const prop of props) comp[prop.name]!(prop.defaultVal);
      return comp;
    };

    comp.resetProps();
    state._rerender = digest;

    if (classMode && nodeElement !== undefined) comp(nodeElement);
    return comp;
  }

  return KapsuleComp as unknown as KapsuleClassCtor;
}
