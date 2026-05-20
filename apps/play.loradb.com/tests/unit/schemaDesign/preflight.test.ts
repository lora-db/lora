/**
 * Tests for the constraint pre-flight scanner. Each scanner runs:
 *  1) a capacity count (small, deterministic)
 *  2) when the universe is small enough, a sample + offending count
 *
 * We stub `run` per scenario to assert the shape of the verdict and
 * the SQL we hand back as the jump query.
 */

import { beforeEach, describe, expect, it, vi } from "vitest";

import type { ConstraintDraft } from "@/lib/schemaDesign/types";
import type { RunOutcome } from "@/lib/db/types";

vi.mock("@/lib/db/client", () => ({
  run: vi.fn(),
}));

import { run } from "@/lib/db/client";
import { runPreflight, SCAN_LIMIT } from "@/lib/schemaDesign/preflight";

type MockRun = ReturnType<typeof vi.fn>;
const mockRun = run as unknown as MockRun;

function okOutcome(columns: string[], rows: unknown[][]): RunOutcome {
  return {
    state: "ok",
    runId: "test",
    startedAt: 0,
    endedAt: 1,
    ms: 1,
    result: {
      columns,
      cellTypes: columns.map(() => "string"),
      rows: rows.map((values) => ({ values })),
      graph: null,
      stats: { nodeCount: 0, relCount: 0, rowCount: rows.length },
    },
  };
}

function errOutcome(message: string): RunOutcome {
  return {
    state: "error",
    runId: "test",
    startedAt: 0,
    endedAt: 1,
    ms: 1,
    message,
  };
}

function uniqueDraft(props: string[] = ["email"]): ConstraintDraft {
  return {
    kind: "UNIQUE",
    entity: "NODE",
    label: "Person",
    properties: props,
    propertyType: "STRING",
    name: "u",
    ifNotExists: true,
  };
}

function existsDraft(): ConstraintDraft {
  return {
    kind: "NOT_NULL",
    entity: "NODE",
    label: "Person",
    properties: ["email"],
    propertyType: "STRING",
    name: "nn",
    ifNotExists: true,
  };
}

function typeDraft(): ConstraintDraft {
  return {
    kind: "PROPERTY_TYPE",
    entity: "NODE",
    label: "Person",
    properties: ["age"],
    propertyType: "INTEGER",
    name: "pt",
    ifNotExists: true,
  };
}

beforeEach(() => {
  mockRun.mockReset();
});

describe("scanUniqueness", () => {
  it("reports ok when no duplicates", async () => {
    mockRun
      .mockResolvedValueOnce(okOutcome(["total"], [[10]]))
      .mockResolvedValueOnce(okOutcome(["k", "c"], []))
      .mockResolvedValueOnce(okOutcome(["offending"], [[0]]));

    const v = await runPreflight(uniqueDraft());
    expect(v.ok).toBe(true);
    expect(v.offending).toBe(0);
    expect(v.capped).toBe(false);
    expect(v.sample).toHaveLength(0);
    expect(v.jumpQuery).toContain("MATCH (n:`Person`)");
  });

  it("reports the offending group count and surfaces sample rows", async () => {
    mockRun
      .mockResolvedValueOnce(okOutcome(["total"], [[10]]))
      .mockResolvedValueOnce(
        okOutcome(
          ["k", "c"],
          [
            ["a@b.com", 3],
            ["c@d.com", 2],
          ],
        ),
      )
      .mockResolvedValueOnce(okOutcome(["offending"], [[2]]));

    const v = await runPreflight(uniqueDraft());
    expect(v.ok).toBe(false);
    expect(v.offending).toBe(2);
    expect(v.sample).toHaveLength(2);
    expect(v.sample[0]).toEqual({ k: "a@b.com", c: 3 });
  });

  it("short-circuits with capped=true when total > SCAN_LIMIT", async () => {
    mockRun.mockResolvedValueOnce(okOutcome(["total"], [[SCAN_LIMIT + 1]]));

    const v = await runPreflight(uniqueDraft());
    expect(v.capped).toBe(true);
    expect(v.ok).toBe(false);
    expect(v.sample).toHaveLength(0);
    expect(mockRun).toHaveBeenCalledTimes(1);
    expect(v.message).toMatch(/scan cap/);
  });

  it("emits composite alias assignments for multi-property UNIQUE", async () => {
    mockRun
      .mockResolvedValueOnce(okOutcome(["total"], [[5]]))
      .mockResolvedValueOnce(okOutcome(["k0", "k1", "c"], []))
      .mockResolvedValueOnce(okOutcome(["offending"], [[0]]));

    await runPreflight(uniqueDraft(["country", "taxId"]));

    const sampleQuery = mockRun.mock.calls[1]?.[0] as string;
    expect(sampleQuery).toContain("n.`country` AS k0");
    expect(sampleQuery).toContain("n.`taxId` AS k1");
    expect(sampleQuery).toContain("count(*) AS c");
  });

  it("returns an error verdict when the engine rejects the capacity query", async () => {
    mockRun.mockResolvedValueOnce(errOutcome("boom"));

    const v = await runPreflight(uniqueDraft());
    expect(v.ok).toBe(false);
    expect(v.capped).toBe(false);
    expect(v.message).toBe("boom");
  });

  it("escapes embedded backticks in label and property identifiers", async () => {
    mockRun
      .mockResolvedValueOnce(okOutcome(["total"], [[1]]))
      .mockResolvedValueOnce(okOutcome([], []))
      .mockResolvedValueOnce(okOutcome(["offending"], [[0]]));

    await runPreflight({
      ...uniqueDraft(),
      label: "Per`son",
      properties: ["e`mail"],
    });

    const sampleQuery = mockRun.mock.calls[1]?.[0] as string;
    expect(sampleQuery).toContain("(n:`Person`)");
    expect(sampleQuery).toContain("n.`email`");
  });
});

describe("scanExistence (IS NOT NULL)", () => {
  it("returns ok=true when nothing is missing", async () => {
    mockRun
      .mockResolvedValueOnce(okOutcome(["total"], [[5]]))
      .mockResolvedValueOnce(okOutcome(["n"], []))
      .mockResolvedValueOnce(okOutcome(["offending"], [[0]]));

    const v = await runPreflight(existsDraft());
    expect(v.ok).toBe(true);
    expect(v.offending).toBe(0);
  });

  it("counts records missing the property", async () => {
    mockRun
      .mockResolvedValueOnce(okOutcome(["total"], [[5]]))
      .mockResolvedValueOnce(okOutcome(["n"], [["row1"], ["row2"]]))
      .mockResolvedValueOnce(okOutcome(["offending"], [[7]]));

    const v = await runPreflight(existsDraft());
    expect(v.ok).toBe(false);
    expect(v.offending).toBe(7);
    expect(v.message).toMatch(/missing/);
  });

  it("short-circuits on capped", async () => {
    mockRun.mockResolvedValueOnce(okOutcome(["total"], [[SCAN_LIMIT + 1]]));
    const v = await runPreflight(existsDraft());
    expect(v.capped).toBe(true);
  });
});

describe("scanPropertyType (IS :: T)", () => {
  it("returns ok=true when all values match the type", async () => {
    mockRun
      .mockResolvedValueOnce(okOutcome(["total"], [[3]]))
      .mockResolvedValueOnce(okOutcome(["value"], []))
      .mockResolvedValueOnce(okOutcome(["offending"], [[0]]));

    const v = await runPreflight(typeDraft());
    expect(v.ok).toBe(true);
  });

  it("emits the IS :: TYPE predicate in the sample query", async () => {
    mockRun
      .mockResolvedValueOnce(okOutcome(["total"], [[3]]))
      .mockResolvedValueOnce(okOutcome(["value"], [["abc"]]))
      .mockResolvedValueOnce(okOutcome(["offending"], [[1]]));

    await runPreflight(typeDraft());

    const sampleQuery = mockRun.mock.calls[1]?.[0] as string;
    expect(sampleQuery).toContain("IS :: INTEGER");
    expect(sampleQuery).toContain("IS NOT NULL");
  });
});
