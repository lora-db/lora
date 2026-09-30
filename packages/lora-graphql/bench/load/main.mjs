// Entry point for `yarn bench:load`: bundle the harness with esbuild (the
// server and client threads are separate entry points, so they cannot run
// through vitest), then run it with the same arguments.

import { build } from "esbuild";

const outdir = new URL(
  "../../node_modules/.cache/lora-graphql-load/",
  import.meta.url,
);
await build({
  entryPoints: ["run", "server", "loadgen"].map(
    (name) => new URL(`${name}.ts`, import.meta.url).pathname,
  ),
  outdir: outdir.pathname,
  bundle: true,
  platform: "node",
  format: "esm",
  target: "node20",
  packages: "external",
  logLevel: "warning",
});
await import(new URL("run.js", outdir).href);
