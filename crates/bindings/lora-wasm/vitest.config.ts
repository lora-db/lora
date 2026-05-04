import { defineConfig } from "vitest/config";

export default defineConfig({
  test: {
    include: ["test/**/*.test.ts"],
    exclude: ["test-browser/**", "node_modules/**", "dist/**"],
    globals: true,
    environment: "node",
    testTimeout: 20_000,
  },
});
