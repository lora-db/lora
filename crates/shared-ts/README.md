# shared-ts

Canonical TypeScript type contract shared by the JS-facing Lora packages
(`lora-node`, `lora-wasm`). The file `types.ts` is the single source of
truth for the public value model, query-result shape, and error types.

Both downstream packages include this directory via TypeScript `rootDirs`
in their own `tsconfig.json`, so imports written as `./types.js` from
`ts/index.ts` resolve to the file here at build time and emit into each
package's own `dist/`. That keeps the published tarballs self-contained
(no cross-package runtime dependency) while preventing drift in the public
TS contract.

This directory intentionally contains no `package.json` — it is a
source-sharing unit, not an npm package.
