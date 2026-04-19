/**
 * Vite config used only to serve the browser smoke-test page and its
 * compiled ESM worker entry during the Playwright run. The WASM binary
 * ships via the `bundler` wasm-pack target, which requires
 * `vite-plugin-wasm` + TLA support at the Vite layer.
 *
 * The crate root is the Vite root so that the references in
 * `examples/browser.html` (`../dist/…`, `../pkg-bundler/…`) resolve
 * against the on-disk layout.
 */

import { defineConfig, type PluginOption } from "vite";
// The plugin packages declare a CJS-style default export that NodeNext
// resolution surfaces as a namespace; unwrap via `.default` when present.
import wasmMod from "vite-plugin-wasm";
import topLevelAwaitMod from "vite-plugin-top-level-await";

type PluginFactory = (...args: never[]) => PluginOption;
const pickDefault = (m: unknown): PluginFactory =>
  (typeof m === "function"
    ? (m as PluginFactory)
    : (m as { default: PluginFactory }).default);

const wasm = pickDefault(wasmMod);
const topLevelAwait = pickDefault(topLevelAwaitMod);

export default defineConfig({
  plugins: [wasm(), topLevelAwait()],
  root: ".",
  server: {
    port: 5180,
    strictPort: true,
    fs: {
      // Allow serving files from the whole crate so dist/, pkg-bundler/ are reachable.
      allow: [".", "./dist", "./pkg-bundler", "./examples"],
    },
  },
  optimizeDeps: {
    // Prevent Vite from pre-bundling the wasm module; the plugin handles it.
    exclude: ["./pkg-bundler/lora_wasm.js"],
  },
  worker: {
    format: "es",
    plugins: () => [wasm(), topLevelAwait()],
  },
});
