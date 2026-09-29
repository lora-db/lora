import { defineConfig } from "vitest/config";

export default defineConfig({
  test: {
    environment: "node",
    globals: true,
    include: ["test/**/*.test.ts"],
    benchmark: { include: ["bench/**/*.bench.ts"] },
    testTimeout: 20_000,
  },
});
