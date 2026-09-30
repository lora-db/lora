import { defineConfig } from "vitest/config";

// LORA_GRAPHQL_VERSION=17 runs the suite on graphql 17 (the `graphql17`
// dev dependency) instead of 16. Both are supported peers, and their
// APIs differ in places the library touches (getVariableValues, the
// execution result shapes), so CI runs the suite on each.
const graphql17 = process.env["LORA_GRAPHQL_VERSION"] === "17";

export default defineConfig({
  resolve: graphql17
    ? { alias: [{ find: /^graphql(\/.*)?$/, replacement: "graphql17$1" }] }
    : {},
  test: {
    environment: "node",
    globals: true,
    include: ["test/**/*.test.ts"],
    benchmark: { include: ["bench/**/*.bench.ts"] },
    testTimeout: 20_000,
  },
});
