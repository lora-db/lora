import { describe, expect, it } from "vitest";
import type { QueryResult } from "@loradb/lora-wasm";

import { adapt } from "@/lib/db/adapter";

describe("adapter — graph activation", () => {
  it("returns no graph when only scalar fields are projected", () => {
    const raw: QueryResult = {
      columns: ["name"],
      rows: [{ name: "Alice" }, { name: "Bob" }],
    };
    const result = adapt(raw);
    expect(result.graph).toBeNull();
    expect(result.stats.nodeCount).toBe(0);
  });

  it("returns no graph when projecting a map-typed property", () => {
    const raw: QueryResult = {
      columns: ["address"],
      rows: [{ address: { street: "Main", city: "NYC" } }],
    };
    const result = adapt(raw);
    expect(result.graph).toBeNull();
  });

  it("activates graph for a real node", () => {
    const raw: QueryResult = {
      columns: ["n"],
      rows: [
        {
          n: {
            kind: "node",
            id: 1,
            labels: ["Person"],
            properties: { name: "Alice" },
          },
        },
      ],
    };
    const result = adapt(raw);
    expect(result.graph).not.toBeNull();
    expect(result.graph!.nodes.length).toBe(1);
  });

  it("does NOT activate graph when projecting nested map that happens to contain string 'node'", () => {
    const raw: QueryResult = {
      columns: ["x"],
      rows: [{ x: { kind: "not-a-node", id: 99 } }],
    };
    const result = adapt(raw);
    expect(result.graph).toBeNull();
  });

  it("stubs endpoint nodes when only a relationship is returned", () => {
    // `RETURN r` produces a link whose source/target ids are not present
    // among the projected columns — without stub endpoints the canvas
    // throws "node not found".
    const raw: QueryResult = {
      columns: ["r"],
      rows: [
        {
          r: {
            kind: "relationship",
            id: 10,
            startId: 1,
            endId: 2,
            type: "KNOWS",
            properties: {},
          },
        },
      ],
    };
    const result = adapt(raw);
    expect(result.graph).not.toBeNull();
    expect(result.graph!.links.length).toBe(1);
    const nodeIds = result.graph!.nodes.map((n) => n.id).sort();
    expect(nodeIds).toEqual([1, 2]);
  });
});
