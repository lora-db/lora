// Helper that builds prop/method shims so an outer kapsule can expose
// pass-through accessors to an inner kapsule (or multiple) without
// rewriting each setter by hand.
//
// LORA: ported from vendor/force-graph/src/kapsule-link.js (MIT, ©
// Vasco Asturiano). Behaviour preserved verbatim; only types added.

type ChainableKapsule = {
  _destructor?: () => void;
  [k: string]: unknown;
};

type KapsuleConstructor = new () => ChainableKapsule;

interface KapsulePropConfig {
  default: unknown;
  onChange: (v: unknown, state: Record<string, ChainableKapsule>) => void;
  triggerUpdate: false;
}

interface KapsuleLinker {
  /** Build a kapsule prop config that forwards the prop to one or more
   *  named inner kapsules on every change. The `default` is read from a
   *  throwaway instance of the inner kapsule type so it stays in sync
   *  with upstream. */
  linkProp: (prop: string) => KapsulePropConfig;
  /** Build a method shim that calls `method(...args)` on every named
   *  inner kapsule and returns either the inner's return value (when
   *  it's not the kapsule itself, i.e. a getter) or `this` for chain
   *  continuity. */
  linkMethod: (
    method: string,
  ) => (state: Record<string, ChainableKapsule>, ...args: unknown[]) => unknown;
}

export default function kapsuleLink(
  kapsulePropNames: string | string[],
  kapsuleType: KapsuleConstructor,
): KapsuleLinker {
  const propNames =
    kapsulePropNames instanceof Array ? kapsulePropNames : [kapsulePropNames];

  // Build a throwaway instance only to extract default values for each
  // prop. Tear it down immediately so we don't leak DOM listeners or
  // simulations from the dummy.
  const dummyK = new kapsuleType();
  dummyK._destructor?.();

  return {
    linkProp(prop: string): KapsulePropConfig {
      return {
        default: (dummyK as Record<string, () => unknown>)[prop]!(),
        onChange(v, state) {
          propNames.forEach((propName) => {
            const inner = state[propName] as Record<
              string,
              (v: unknown) => unknown
            >;
            inner[prop]!.call(state[propName], v);
          });
        },
        triggerUpdate: false,
      };
    },
    linkMethod(method: string) {
      return function (this: unknown, state, ...args: unknown[]): unknown {
        const returnVals: unknown[] = [];
        propNames.forEach((propName) => {
          const kapsuleInstance = state[propName] as Record<
            string,
            (...a: unknown[]) => unknown
          > &
            ChainableKapsule;
          const returnVal = kapsuleInstance[method]!.apply(
            kapsuleInstance,
            args,
          );
          if (returnVal !== kapsuleInstance) {
            returnVals.push(returnVal);
          }
        });
        return returnVals.length ? returnVals[0] : this;
      };
    },
  };
}
