/**
 * Build-time version constants.
 *
 * The `lora-wasm` version is the binary that powers the WASM database
 * engine running in the browser — useful for spotting stale CDN copies
 * or pinning bug reports to a specific build. It is read from the
 * workspace's `@loradb/lora-wasm/package.json` by `next.config.mjs` and
 * baked into the static bundle via `NEXT_PUBLIC_LORA_WASM_VERSION`.
 *
 * Falls back to `"unknown"` if the env var was not set at build time so
 * downstream UI can render without a guard.
 */

export const LORA_WASM_VERSION =
  process.env.NEXT_PUBLIC_LORA_WASM_VERSION ?? "unknown";
