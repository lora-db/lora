// Fails when the npm tarball would be missing files the package needs at
// runtime. Guards against wasm-pack's `pkg-*/.gitignore` (containing `*`)
// silently dropping the WASM output from `npm pack` / `npm publish`.
//
// It then installs the packed tarball into a scratch project and imports
// the entry points that load under Node, the same way a user would.
//
// Run after `yarn run build`, from crates/bindings/lora-wasm.

import { execFileSync } from "node:child_process";
import { mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

const pkg = JSON.parse(
  readFileSync(new URL("../package.json", import.meta.url), "utf8"),
);

const required = new Set();
const add = (p) => required.add(p.replace(/^\.\//, ""));
add(pkg.main);
add(pkg.types);
for (const entry of Object.values(pkg.exports)) {
  for (const target of Object.values(entry)) add(target);
}
// Bundled offline docs (scripts/bundle-package-docs.mjs, run by `build`).
add("docs/README.md");
add("docs/getting-started/wasm.md");
for (const dir of ["pkg-node", "pkg-bundler", "pkg-web"]) {
  add(`${dir}/lora_wasm.js`);
  add(`${dir}/lora_wasm.d.ts`);
  add(`${dir}/lora_wasm_bg.wasm`);
}

const out = execFileSync(
  "npm",
  ["pack", "--dry-run", "--json", "--ignore-scripts"],
  {
    encoding: "utf8",
  },
);
const packed = new Set(JSON.parse(out)[0].files.map((f) => f.path));

const missing = [...required].filter((p) => !packed.has(p));

if (missing.length > 0) {
  console.error("npm tarball is missing required files:");
  for (const p of missing) console.error(`  - ${p}`);
  console.error("Check for a stray pkg-*/.gitignore left by wasm-pack.");
  process.exit(1);
}

console.log(
  `verify-pack: all ${required.size} required files are in the tarball`,
);

// `/worker` needs a Web Worker global and `/bundler` needs a bundler (or
// Node's experimental WASM modules), so only these are imported here.
const NODE_ENTRIES = ["", "/web", "/worker-client"];

const scratch = mkdtempSync(join(tmpdir(), "lora-wasm-pack-"));
try {
  const run = (cmd, args, cwd) =>
    execFileSync(cmd, args, { cwd, encoding: "utf8" });
  const tgz = JSON.parse(
    run(
      "npm",
      ["pack", "--json", "--ignore-scripts", "--pack-destination", scratch],
      process.cwd(),
    ),
  )[0].filename;
  writeFileSync(
    join(scratch, "package.json"),
    '{"name":"verify-pack","private":true,"type":"module"}',
  );
  run(
    "npm",
    ["install", "--no-audit", "--no-fund", "--ignore-scripts", `./${tgz}`],
    scratch,
  );
  for (const entry of NODE_ENTRIES) {
    const spec = `${pkg.name}${entry}`;
    run(
      "node",
      ["--input-type=module", "-e", `await import(${JSON.stringify(spec)})`],
      scratch,
    );
    console.log(`verify-pack: import("${spec}") ok`);
  }
} finally {
  rmSync(scratch, { recursive: true, force: true });
}
