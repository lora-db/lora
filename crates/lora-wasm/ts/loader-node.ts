/**
 * Node-specific loader. The wasm-pack `--target nodejs` output is CommonJS,
 * so we wrap it with `createRequire` to stay ESM-compatible.
 */

import { createRequire } from "node:module";
import type { WasmDatabase as NativeWasmDatabase } from "../pkg-node/lora_wasm.js";

export interface NativeModule {
  WasmDatabase: new () => NativeWasmDatabase;
  init: () => void;
}

const require = createRequire(import.meta.url);
// The pkg-node path is stable because `wasm-pack build --out-dir pkg-node`
// writes it next to this file's parent.
const mod = require("../pkg-node/lora_wasm.js") as NativeModule;

export const WasmDatabase = mod.WasmDatabase;
export const init = mod.init;
