// Empty shim for browser builds.
//
// Used by `next.config.mjs` to neutralize two unreachable-on-the-browser
// modules at bundle time:
//
//  * `@loradb/lora-wasm/dist/loader-node.js` — pulls `node:module`. The
//    real WASM loader for the browser comes from the Worker / bundler
//    pkg path, which `createDatabase` reaches at runtime. Webpack
//    needs the named exports `WasmDatabase` + `init` to satisfy the
//    static ESM check; their runtime values are never read because
//    the worker / bundler path is taken first.
//  * `react-responsive-carousel` — peer dep of glide-data-grid that
//    isn't installed. It's only required by the image overlay editor,
//    which is never rendered for our table cell kinds.
//
// Both should disappear once the upstream packaging is fixed.

export function Carousel() {
  return null;
}
export const WasmDatabase = null;
export const init = null;
export default Carousel;
