import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const mocks = vi.hoisted(() => ({
  getDB: vi.fn(),
}));

vi.mock("@/lib/persistence/idb", () => ({
  getDB: mocks.getDB,
}));

import { readAuto, writeAuto } from "@/lib/persistence/autoSnapshot";

describe("autoSnapshot persistence", () => {
  const originalWindow = globalThis.window;
  let warnSpy: ReturnType<typeof vi.spyOn>;

  beforeEach(() => {
    mocks.getDB.mockReset();
    warnSpy = vi.spyOn(console, "warn").mockImplementation(() => {});
    Object.defineProperty(globalThis, "window", {
      configurable: true,
      value: {},
    });
  });

  afterEach(() => {
    Object.defineProperty(globalThis, "window", {
      configurable: true,
      value: originalWindow,
    });
    warnSpy.mockRestore();
  });

  it("propagates read failures so auto-restore can notify the user", async () => {
    mocks.getDB.mockRejectedValueOnce(new Error("blocked"));

    await expect(readAuto()).rejects.toThrow("blocked");
  });

  it("classifies quota failures while writing", async () => {
    mocks.getDB.mockResolvedValueOnce({
      put: vi
        .fn()
        .mockRejectedValueOnce(new DOMException("full", "QuotaExceededError")),
    });

    await expect(writeAuto(new Uint8Array([1, 2, 3]))).resolves.toEqual({
      ok: false,
      reason: "quota-exceeded",
    });
  });
});
