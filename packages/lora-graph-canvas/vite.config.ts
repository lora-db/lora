import { defineConfig } from "vite";
import { resolve } from "node:path";
import react from "@vitejs/plugin-react";
import dts from "vite-plugin-dts";

export default defineConfig({
  plugins: [
    react(),
    dts({ entryRoot: "src", include: ["src/**/*"], rollupTypes: false }),
  ],
  build: {
    target: "es2022",
    sourcemap: true,
    lib: {
      entry: resolve(__dirname, "src/index.ts"),
      formats: ["es", "cjs"],
      fileName: (format) => `index.${format === "es" ? "js" : "cjs"}`,
    },
    rollupOptions: {
      // force-graph is no longer external — the in-tree port at
      // src/engines/force-graph-2d/ gets bundled into the published
      // artifact.
      external: [
        "react",
        "react-dom",
        "react/jsx-runtime",
        "3d-force-graph",
        "three",
        /^three\//,
      ],
    },
  },
});
