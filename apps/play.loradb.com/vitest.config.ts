/**
 * Vitest config for unit-level tests (zustand slice logic, util fns).
 *
 * Lives alongside Playwright (`tests/e2e`) but only picks up files inside
 * `tests/unit` so the two runners stay separated. Path alias mirrors the
 * Next.js `tsconfig.json` so `@/lib/...` imports resolve identically.
 */

import path from "node:path";
import { fileURLToPath } from "node:url";

import { defineConfig } from "vitest/config";

const here = path.dirname(fileURLToPath(import.meta.url));

export default defineConfig({
  resolve: {
    alias: {
      "@": here,
    },
  },
  test: {
    environment: "node",
    include: ["tests/unit/**/*.test.ts"],
  },
});
