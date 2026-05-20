/**
 * Tests for the imperative schema-design actions. We mock the DB,
 * notifications, and the Zustand store; assertions cover the DDL the
 * action emits, friendly-error translation, and the refresh-ticket
 * race.
 */

import { beforeEach, describe, expect, it, vi } from "vitest";

import type { ConstraintDraft, IndexDraft } from "@/lib/schemaDesign/types";

vi.mock("@mantine/notifications", () => ({
  notifications: { show: vi.fn() },
}));

vi.mock("@/lib/db/schemaDesign", () => ({
  runDDL: vi.fn(),
  fetchSchemaDesignSnapshot: vi.fn(),
  SchemaDesignError: class extends Error {},
}));

// Minimal store mock. The action layer only touches the setters and
// reads via `useStore.getState()`.
const setSchemaDesign = vi.fn();
const setSchemaDesignError = vi.fn();
const setSchemaDesignRefreshing = vi.fn();
const storeState = {
  setSchemaDesign,
  setSchemaDesignError,
  setSchemaDesignRefreshing,
};

vi.mock("@/lib/state/store", () => ({
  useStore: { getState: () => storeState },
}));

// runActiveTab pulls in the WASM client; stub the event constant so
// the action module's `attachSchemaDesignMutationListener` import
// doesn't cascade into loading WASM during the test.
vi.mock("@/lib/actions/runActiveTab", () => ({
  LORADB_MUTATION_EVENT: "loradb:mutation",
}));

import { notifications } from "@mantine/notifications";
import { fetchSchemaDesignSnapshot, runDDL } from "@/lib/db/schemaDesign";
import {
  createConstraint,
  createIndex,
  dropConstraint,
  dropIndex,
  refreshSchemaDesign,
  updateConstraint,
  updateIndex,
} from "@/lib/actions/schemaDesignActions";

const mockRunDDL = runDDL as ReturnType<typeof vi.fn>;
const mockFetch = fetchSchemaDesignSnapshot as ReturnType<typeof vi.fn>;
const mockNotify = (notifications.show as unknown) as ReturnType<typeof vi.fn>;

const indexDraft: IndexDraft = {
  kind: "RANGE",
  entity: "NODE",
  label: "Person",
  properties: ["email"],
  name: "idx_person_email",
  ifNotExists: true,
};

const constraintDraft: ConstraintDraft = {
  kind: "UNIQUE",
  entity: "NODE",
  label: "Person",
  properties: ["email"],
  propertyType: "STRING",
  name: "unique_person_email",
  ifNotExists: true,
};

beforeEach(() => {
  mockRunDDL.mockReset();
  mockFetch.mockReset().mockResolvedValue({ indexes: [], constraints: [] });
  mockNotify.mockReset();
  setSchemaDesign.mockReset();
  setSchemaDesignError.mockReset();
  setSchemaDesignRefreshing.mockReset();
});

describe("createIndex", () => {
  it("emits the wizard's DDL and refreshes on success", async () => {
    mockRunDDL.mockResolvedValueOnce(undefined);

    const ok = await createIndex(indexDraft);

    expect(ok).toBe(true);
    expect(mockRunDDL).toHaveBeenCalledWith(
      expect.stringContaining("CREATE RANGE INDEX `idx_person_email`"),
    );
    expect(mockFetch).toHaveBeenCalled();
    expect(mockNotify).toHaveBeenCalledWith(
      expect.objectContaining({ color: "green", title: "Index created" }),
    );
  });

  it("surfaces a translated friendly error on failure", async () => {
    mockRunDDL.mockRejectedValueOnce(new Error("[22N73] already exists"));

    const ok = await createIndex(indexDraft);

    expect(ok).toBe(false);
    expect(mockNotify).toHaveBeenCalledWith(
      expect.objectContaining({
        color: "red",
        title: "A range index on this property already exists",
      }),
    );
  });
});

describe("createConstraint", () => {
  it("emits the constraint DDL and refreshes", async () => {
    mockRunDDL.mockResolvedValueOnce(undefined);

    const ok = await createConstraint(constraintDraft);

    expect(ok).toBe(true);
    expect(mockRunDDL).toHaveBeenCalledWith(
      expect.stringContaining("CREATE CONSTRAINT `unique_person_email`"),
    );
    expect(mockNotify).toHaveBeenCalledWith(
      expect.objectContaining({ title: "Constraint created" }),
    );
  });

  it("translates an engine code in the error path", async () => {
    mockRunDDL.mockRejectedValueOnce(new Error("[22N79] duplicates"));

    const ok = await createConstraint(constraintDraft);

    expect(ok).toBe(false);
    expect(mockNotify).toHaveBeenCalledWith(
      expect.objectContaining({ title: "Duplicate values already exist" }),
    );
  });
});

describe("updateIndex", () => {
  it("issues DROP then CREATE for the new draft", async () => {
    mockRunDDL.mockResolvedValueOnce(undefined).mockResolvedValueOnce(undefined);

    const ok = await updateIndex("idx_person_email", {
      ...indexDraft,
      properties: ["email", "tenant_id"],
      name: "idx_person_email_tenant",
    });

    expect(ok).toBe(true);
    expect(mockRunDDL).toHaveBeenNthCalledWith(
      1,
      "DROP INDEX `idx_person_email` IF EXISTS",
    );
    expect(mockRunDDL).toHaveBeenNthCalledWith(
      2,
      expect.stringContaining("CREATE RANGE INDEX `idx_person_email_tenant`"),
    );
    expect(mockNotify).toHaveBeenCalledWith(
      expect.objectContaining({ color: "green", title: "Index updated" }),
    );
  });

  it("reports a partial failure loudly when DROP succeeded but CREATE failed", async () => {
    mockRunDDL
      .mockResolvedValueOnce(undefined)
      .mockRejectedValueOnce(new Error("create broke"));

    const ok = await updateIndex("idx_person_email", indexDraft);

    expect(ok).toBe(false);
    expect(mockNotify).toHaveBeenCalledWith(
      expect.objectContaining({
        color: "red",
        title: "Index update partially failed",
        autoClose: false,
      }),
    );
  });

  it("aborts cleanly when DROP fails — no CREATE attempted", async () => {
    mockRunDDL.mockRejectedValueOnce(new Error("drop broke"));

    const ok = await updateIndex("idx_person_email", indexDraft);

    expect(ok).toBe(false);
    expect(mockRunDDL).toHaveBeenCalledTimes(1);
  });
});

describe("updateConstraint", () => {
  it("issues DROP then CREATE", async () => {
    mockRunDDL.mockResolvedValueOnce(undefined).mockResolvedValueOnce(undefined);

    const ok = await updateConstraint("unique_person_email", constraintDraft);

    expect(ok).toBe(true);
    expect(mockRunDDL).toHaveBeenNthCalledWith(
      1,
      "DROP CONSTRAINT `unique_person_email` IF EXISTS",
    );
    expect(mockRunDDL).toHaveBeenNthCalledWith(
      2,
      expect.stringContaining("CREATE CONSTRAINT `unique_person_email`"),
    );
    expect(mockNotify).toHaveBeenCalledWith(
      expect.objectContaining({ title: "Constraint updated" }),
    );
  });
});

describe("dropIndex / dropConstraint", () => {
  it("dropIndex emits a DROP statement with IF EXISTS", async () => {
    mockRunDDL.mockResolvedValueOnce(undefined);

    await dropIndex("idx_person_email");

    expect(mockRunDDL).toHaveBeenCalledWith(
      "DROP INDEX `idx_person_email` IF EXISTS",
    );
  });

  it("dropConstraint emits a DROP statement with IF EXISTS", async () => {
    mockRunDDL.mockResolvedValueOnce(undefined);

    await dropConstraint("unique_person_email");

    expect(mockRunDDL).toHaveBeenCalledWith(
      "DROP CONSTRAINT `unique_person_email` IF EXISTS",
    );
  });
});

describe("refreshSchemaDesign", () => {
  it("calls fetch and stores the snapshot", async () => {
    mockFetch.mockResolvedValueOnce({
      indexes: [{ name: "idx", kind: "RANGE", entity: "NODE", labelsOrTypes: ["Person"], properties: ["email"], state: "online", populationPercent: 100, owned: false }],
      constraints: [],
    });

    await refreshSchemaDesign();

    expect(setSchemaDesign).toHaveBeenCalledWith(
      expect.objectContaining({ indexes: expect.any(Array), constraints: [] }),
    );
    expect(setSchemaDesignRefreshing).toHaveBeenCalledWith(true);
    expect(setSchemaDesignRefreshing).toHaveBeenLastCalledWith(false);
  });

  it("ticket race: only the most-recent fetch lands", async () => {
    let resolveFirst!: (v: unknown) => void;
    mockFetch
      .mockImplementationOnce(
        () => new Promise((resolve) => (resolveFirst = resolve)),
      )
      .mockResolvedValueOnce({ indexes: [], constraints: [] });

    const first = refreshSchemaDesign();
    const second = refreshSchemaDesign();
    await second;
    resolveFirst({ indexes: ["stale"], constraints: ["stale"] });
    await first;

    // setSchemaDesign should have been called exactly once — by the
    // second fetch. The first one's resolution is discarded because
    // the ticket changed.
    expect(setSchemaDesign).toHaveBeenCalledTimes(1);
    expect(setSchemaDesign).toHaveBeenCalledWith({
      indexes: [],
      constraints: [],
    });
  });

  it("surfaces a notification when the fetch fails", async () => {
    mockFetch.mockRejectedValueOnce(new Error("boom"));

    await refreshSchemaDesign();

    expect(setSchemaDesignError).toHaveBeenCalled();
    expect(mockNotify).toHaveBeenCalledWith(
      expect.objectContaining({
        color: "red",
        title: "Couldn't load the schema catalog",
      }),
    );
  });
});
