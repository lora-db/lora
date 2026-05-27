# Security Policy

## Supported versions

Lora is pre-1.0. Security fixes land on `main` and are included in the next
tagged release. Older tags are **not** patched.

| Version           | Supported |
| ----------------- | --------- |
| `main` (unreleased) | ✅        |
| Latest `v0.x.y` tag | ✅        |
| Older tags          | ❌        |

## Reporting a vulnerability

**Please do not open a public GitHub issue for security vulnerabilities.**

Report suspected vulnerabilities privately via GitHub's
[Security Advisories](https://docs.github.com/code-security/security-advisories/guidance-on-reporting-and-writing-information-about-vulnerabilities/privately-reporting-a-security-vulnerability)
feature on this repository. From the repo's **Security** tab, click
**Report a vulnerability**.

Alternatively, email the maintainers at **security@loradb.com** with:

- A description of the issue and the impact you believe it has.
- Reproduction steps, a proof-of-concept, or a minimal Cypher query / HTTP
  request that triggers the behavior.
- The commit SHA or release tag you tested against.
- Your disclosure timeline preferences.

You should receive an acknowledgement within **3 business days**. We aim to
provide an initial assessment within **10 business days** and will keep you
informed as a fix is developed and released.

## Scope

In scope:

- `lora-server` HTTP endpoints (`POST /query`, `GET /health`) and
  opt-in admin endpoints when snapshots or WAL are enabled.
- The Cypher parser, analyzer, compiler, and executor (panics, denial of
  service, memory corruption, sandbox escapes from expression evaluation).
- The language bindings: `lora-node`, `lora-wasm`, `lora-python`,
  `lora-go`, and `lora-ruby`.
- The shared `lora-ffi` C ABI (consumed by `lora-go` and any
  third-party cgo consumer).
- The release artifacts and checksums published on the Releases page.

Out of scope:

- Known limitations documented in `README.md` and `loradb.com/docs/limitations`
  (for example no built-in HTTP authentication, no TLS termination, no
  clustering, single-process write serialization, and surface-dependent
  persistence controls). These are *design decisions*, not vulnerabilities by
  themselves.
- Issues that require running `lora-server` or opt-in admin endpoints on an
  untrusted network without an authenticated reverse proxy in front. The docs
  explicitly call that deployment unsupported.
- Third-party dependencies unless the bug is reachable through Lora's API
  surface.
- The `apps/loradb.com` Docusaurus site (report documentation-site issues as
  normal GitHub issues).

## Disclosure

We follow **coordinated disclosure**. Once a fix is ready we will:

1. Cut a new release containing the fix.
2. Publish a GitHub Security Advisory with a CVE if the severity warrants it.
3. Credit the reporter (unless they request anonymity).

Please give us a reasonable window (typically 90 days) before public
disclosure.
