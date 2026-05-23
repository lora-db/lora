import { beforeEach, describe, expect, it, vi } from "vitest";

describe("getDB", () => {
  beforeEach(() => {
    vi.resetModules();
    vi.unstubAllGlobals();
  });

  it("retries after a cached open failure", async () => {
    const fakeDb = { close: vi.fn(), objectStoreNames: { contains: vi.fn() } };
    const openDB = vi
      .fn()
      .mockRejectedValueOnce(new Error("blocked"))
      .mockResolvedValueOnce(fakeDb);

    vi.doMock("idb", () => ({
      openDB,
      deleteDB: vi.fn(),
    }));
    vi.stubGlobal("window", {});

    const { getDB } = await import("@/lib/persistence/idb");

    await expect(getDB()).rejects.toThrow("blocked");
    await expect(getDB()).resolves.toBe(fakeDb);
    expect(openDB).toHaveBeenCalledTimes(2);
  });
});
