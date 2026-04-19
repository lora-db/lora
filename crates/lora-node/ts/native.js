// Native module loader. Picks the right platform-specific prebuilt binary if
// present, otherwise falls back to the dev build produced by `napi build`.
//
// napi-rs appends an ABI suffix on some platforms:
//   - Linux GNU:   lora-node.linux-x64-gnu.node
//   - Linux musl:  lora-node.linux-x64-musl.node
//   - Windows:     lora-node.win32-x64-msvc.node
//   - macOS:       lora-node.darwin-arm64.node  (no suffix)
//   - FreeBSD:     lora-node.freebsd-x64.node
//
// We build the list of candidate filenames accordingly and try each one.

"use strict";

const { platform, arch } = process;
const { existsSync, readFileSync } = require("node:fs");
const { join } = require("node:path");
const { execSync } = require("node:child_process");

function isMuslLibc() {
  // glibc `ldd --version` mentions GNU; musl's mentions "musl".
  try {
    const out = execSync("ldd --version", { stdio: ["ignore", "pipe", "pipe"] })
      .toString()
      .toLowerCase();
    return out.includes("musl");
  } catch {
    // When ldd can't be run, fall back to Node's process report which
    // exposes `glibcVersionRuntime` only on glibc systems.
    try {
      const header = process.report?.getReport()?.header;
      if (header && "glibcVersionRuntime" in header) return false;
    } catch {
      /* ignore */
    }
    // Default to glibc — the common case on CI Linux runners.
    return false;
  }
}

function candidateTriples() {
  if (platform === "linux") {
    const libc = isMuslLibc() ? "musl" : "gnu";
    return [`${platform}-${arch}-${libc}`, `${platform}-${arch}`];
  }
  if (platform === "win32") {
    return [`${platform}-${arch}-msvc`, `${platform}-${arch}`];
  }
  // darwin, freebsd, etc — napi doesn't add an ABI suffix.
  return [`${platform}-${arch}`];
}

function loadNative() {
  const triples = candidateTriples();

  // 1. Local dev build at the crate root (produced by `napi build`).
  const localCandidates = [
    ...triples.map((t) => `lora-node.${t}.node`),
    "lora.node",
    "lora-node.node",
  ];
  for (const name of localCandidates) {
    const candidate = join(__dirname, "..", name);
    if (existsSync(candidate)) {
      return require(candidate);
    }
  }

  // 2. Platform-specific npm subpackage (future: @lora/lora-node-<triple>).
  for (const triple of triples) {
    try {
      return require(`@lora/lora-node-${triple}`);
    } catch {
      // fall through to the next triple / final error.
    }
  }

  throw new Error(
    `lora-node: no native binary found for ${platform}-${arch}. ` +
      `Tried: ${localCandidates.join(", ")}. ` +
      "Run `npm run build` in the crate directory to produce a local build.",
  );
}

module.exports = loadNative();
