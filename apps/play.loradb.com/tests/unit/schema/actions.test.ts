import { beforeEach, describe, expect, it, vi } from "vitest";

vi.mock("@mantine/notifications", () => ({
  notifications: { show: vi.fn() },
}));

vi.mock("@/lib/db/schema", () => ({
  introspect: vi.fn(),
}));

vi.mock("@/lib/db/client", () => ({
  run: vi.fn(),
}));

vi.mock("@/lib/actions/runActiveTab", () => ({
  LORADB_MUTATION_EVENT: "loradb:mutation",
}));

const setSchema = vi.fn();
const setRefreshing = vi.fn();
const storeState = {
  setSchema,
  setRefreshing,
};

vi.mock("@/lib/state/store", () => ({
  useStore: { getState: () => storeState },
}));

import { introspect } from "@/lib/db/schema";
import { refreshSchema } from "@/lib/actions/schemaActions";

const mockIntrospect = introspect as ReturnType<typeof vi.fn>;

describe("schemaActions", () => {
  beforeEach(() => {
    mockIntrospect.mockReset();
    setSchema.mockReset();
    setRefreshing.mockReset();
  });

  it("drops stale refresh results when a newer refresh completes first", async () => {
    let resolveOld!: (value: unknown) => void;
    const oldSnapshot = { labels: ["old"], relTypes: [], propertyKeys: [] };
    const newSnapshot = { labels: ["new"], relTypes: [], propertyKeys: [] };

    mockIntrospect
      .mockReturnValueOnce(new Promise((resolve) => (resolveOld = resolve)))
      .mockResolvedValueOnce(newSnapshot);

    const oldRun = refreshSchema();
    await refreshSchema();
    resolveOld(oldSnapshot);
    await oldRun;

    expect(setSchema).toHaveBeenCalledTimes(1);
    expect(setSchema).toHaveBeenCalledWith(newSnapshot);
    expect(setRefreshing).toHaveBeenLastCalledWith(false);
  });
});
