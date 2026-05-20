import { describe, expect, it } from "vitest";

import { translateError } from "@/lib/schemaDesign/errorTranslate";

describe("translateError", () => {
  it("matches known codes", () => {
    expect(translateError("[22N65] something something").code).toBe("22N65");
    expect(translateError("[22N73] index exists").suggestedAction).toBe("useExisting");
  });

  it("falls back via substring sniffing", () => {
    const friendly = translateError("Constraint already exists on schema");
    expect(friendly.title).toBe("An identical constraint already exists");
  });

  it("returns a generic envelope for unknown errors", () => {
    const friendly = translateError("kaboom");
    expect(friendly.title).toBe("The database rejected the change");
    expect(friendly.body).toBe("kaboom");
  });

  it("handles bare codes without brackets", () => {
    expect(translateError("22N79 duplicates").code).toBe("22N79");
  });

  it("translates 22N70 to an existing-index hint", () => {
    const f = translateError("[22N70] equivalent index already exists");
    expect(f.title).toBe("An identical index already exists");
    expect(f.suggestedAction).toBe("useExisting");
  });

  it("translates 22N80 (write-time uniqueness conflict)", () => {
    const f = translateError("[22N80] backing index conflict");
    expect(f.suggestedAction).toBe("fixData");
  });

  it("translates 22N90 (unsupported property type)", () => {
    expect(translateError("[22N90] unsupported type").code).toBe("22N90");
  });

  it("translates 42N51 (not found)", () => {
    const f = translateError("[42N51] missing");
    expect(f.title).toBe("Index or constraint not found");
  });
});
