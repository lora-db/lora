// A caller's `context.signal` need not be the platform's own AbortSignal:
// graphql-yoga hands its request's signal from whatwg-node, which passes
// `instanceof AbortSignal` but fails the brand check Node 24.22's
// `AbortSignal.any` makes ("signals[0] is not of type AbortSignal").
import { describe, expect, test } from "vitest";
import { createTestLoraGraphQL } from "../src/testing.js";
import { anySignal, nativeSignal } from "../src/execute/budget.js";

/** An `EventTarget` with `aborted`, `reason` and `abort` events. */
class ForeignSignal extends EventTarget {
  aborted = false;
  reason: unknown = undefined;
  onabort: ((event: Event) => void) | null = null;
  abort(reason: unknown = new Error("aborted by the caller")): void {
    if (this.aborted) return;
    this.aborted = true;
    this.reason = reason;
    this.dispatchEvent(new Event("abort"));
  }
}
// Like whatwg-node's CustomAbortControllerSignal.
Object.setPrototypeOf(ForeignSignal.prototype, AbortSignal.prototype);
const foreign = () =>
  new ForeignSignal() as unknown as AbortSignal & {
    abort(reason?: unknown): void;
  };

const J = "type Claims @jwt { sub: String! }\n";
const typeDefs =
  J +
  `type F @node @mutation {
    key: String! @key
    name: String!
    slow: Int @cypher(statement: "UNWIND range(1, 10000) AS i UNWIND range(1, 10000) AS j WITH i WHERE toString(i + j) = 'x' RETURN count(i) AS slow", columnName: "slow")
  }`;

describe("a signal that is not the platform's own", () => {
  test("passes instanceof but not AbortSignal.any", () => {
    const s = foreign();
    expect(s instanceof AbortSignal).toBe(true);
    if (typeof AbortSignal.any === "function") {
      // Node 24.22 throws here; older Nodes accepted it. Either way the
      // helpers below must not.
      try {
        AbortSignal.any([s]);
      } catch (err) {
        expect(String(err)).toMatch(/AbortSignal/);
      }
    }
  });

  test("anySignal and nativeSignal combine it with a native one", () => {
    const s = foreign();
    const timeout = new AbortController();
    const both = anySignal(s, timeout.signal);
    const own = nativeSignal(s)!;
    expect(nativeSignal(s)).toBe(own);
    expect(nativeSignal(timeout.signal)).toBe(timeout.signal);
    expect(both.aborted).toBe(false);
    s.abort("stop");
    expect(both.aborted).toBe(true);
    expect(both.reason).toBe("stop");
    expect(own.aborted).toBe(true);
    expect(own.reason).toBe("stop");
  });

  test("queries, mutations and transactions run under it", async () => {
    const t = await createTestLoraGraphQL({
      typeDefs,
      seed: "CREATE (:F {key: 'f1', name: 'sun'})",
    });
    try {
      const signal = foreign();
      const read = await t.run(
        `{ fs { key name } f(key: "f1") { name } }`,
        {},
        {
          signal,
        },
      );
      expect(read.errors).toBeUndefined();
      expect(read.data).toEqual({
        fs: [{ key: "f1", name: "sun" }],
        f: { name: "sun" },
      });
      const write = await t.run(
        `mutation { createFs(input: [{ key: "f2", name: "moon" }]) { fs { key } } }`,
        {},
        { signal },
      );
      expect(write.errors).toBeUndefined();
      expect(write.data).toEqual({ createFs: { fs: [{ key: "f2" }] } });
    } finally {
      t.close();
    }
  });

  test("aborts an operation when it aborts", async () => {
    const t = await createTestLoraGraphQL({
      typeDefs,
      seed: "CREATE (:F {key: 'f1', name: 'sun'})",
    });
    try {
      const before = foreign();
      before.abort(new Error("gone"));
      const early = await t.run(`{ fs { key } }`, {}, { signal: before });
      expect(early.data).toBeNull();
      expect(early.errors?.length).toBe(1);

      const signal = foreign();
      const started = performance.now();
      const pending = t.run(`{ f(key: "f1") { slow } }`, {}, { signal });
      setTimeout(() => signal.abort(new Error("gone")), 50);
      const late = await pending;
      expect(late.errors?.length).toBe(1);
      expect(performance.now() - started).toBeLessThan(5000);
    } finally {
      t.close();
    }
  });
});
