// Best-effort extra wasm-opt -Oz pass over the wasm-pack output.
//
// wasm-pack already runs `wasm-opt -O` against the release build, but
// `-Oz` (size-prioritised) typically shaves another 10–20% off the
// payload at the cost of a single extra second of build time. We do
// this in a postbuild step rather than baking it into the pack
// configuration because (a) older wasm-pack versions ignore unknown
// opt flags, and (b) wasm-opt is not always on the developer's PATH
// (it ships with binaryen) — failing the build for a missing optimiser
// would be worse than just shipping the slightly-larger blob.

import { existsSync, statSync } from "node:fs";
import { spawnSync } from "node:child_process";
import { resolve, dirname } from "node:path";
import { fileURLToPath } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));
const wasm = resolve(here, "..", "wasm", "lora_query_wasm_bg.wasm");

if (!existsSync(wasm)) {
  // wasm-pack output not present — nothing to do. Don't fail the build;
  // this script is opportunistic.
  process.exit(0);
}

const which = spawnSync("which", ["wasm-opt"], { encoding: "utf8" });
if (which.status !== 0) {
  console.log("[wasm-opt-size] wasm-opt not found on PATH — skipping -Oz pass");
  process.exit(0);
}

const before = statSync(wasm).size;
const result = spawnSync(
  "wasm-opt",
  ["-Oz", "--strip-debug", "--vacuum", wasm, "-o", wasm],
  { stdio: "inherit" },
);
if (result.status !== 0) {
  console.log("[wasm-opt-size] wasm-opt exited non-zero — leaving original");
  process.exit(0);
}
const after = statSync(wasm).size;
const pct = ((1 - after / before) * 100).toFixed(1);
console.log(
  `[wasm-opt-size] ${(before / 1024).toFixed(1)} KiB → ${(after / 1024).toFixed(1)} KiB (-${pct}%)`,
);
