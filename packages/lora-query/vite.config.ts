import { defineConfig } from "vite";
import { resolve } from "node:path";
import react from "@vitejs/plugin-react";
import dts from "vite-plugin-dts";
import wasm from "vite-plugin-wasm";
import topLevelAwait from "vite-plugin-top-level-await";

export default defineConfig({
  plugins: [
    react(),
    wasm(),
    topLevelAwait(),
    dts({ entryRoot: "src", include: ["src/**/*"], rollupTypes: false }),
  ],
  build: {
    target: "es2022",
    sourcemap: true,
    lib: {
      entry: {
        index: resolve(__dirname, "src/index.ts"),
        parser: resolve(__dirname, "src/parser.ts"),
      },
      // ESM-only. The WASM bundle uses top-level await, which the
      // rollup CJS renderer rejects ("Module format 'cjs' does not
      // support top-level await") regardless of the vite-plugin-top-
      // level-await transform — that plugin only rewrites ES output.
      // Since the package ships WASM (which needs bundler-side wasm
      // support anyway) and `engines.node >= 20`, dropping the CJS
      // build is fine; modern consumers all handle ESM, and Node 22+
      // can `require()` ESM directly.
      formats: ["es"],
      fileName: (_format, name) => `${name}.js`,
    },
    rollupOptions: {
      external: [
        "react",
        "react-dom",
        "react/jsx-runtime",
        /^@codemirror\//,
        /^@lezer\//,
      ],
    },
  },
});
