---
title: Contact the LoraDB Team
description: How to reach the LoraDB team and community — bug reports, design discussion, security disclosure, and business inquiries.
---

{/*
  Per-page JSON-LD. ContactPage with mainEntity pointing at the
  Organization gives crawlers a clean "this is where you reach LoraDB"
  signal, and the explicit `contactPoint` array surfaces the same
  routes the visible page lists (support, security, press). All
  facts are truthful — only routes that exist on the visible page
  appear here.
*/}

<script
  type="application/ld+json"
  dangerouslySetInnerHTML={{
    __html: JSON.stringify({
      "@context": "https://schema.org",
      "@graph": [
        {
          "@type": "ContactPage",
          "@id": "https://loradb.com/contact#contactpage",
          url: "https://loradb.com/contact",
          name: "Contact the LoraDB team",
          description:
            "How to reach the LoraDB team and community — bug reports, design discussion, security disclosure, and business inquiries.",
          isPartOf: { "@id": "https://loradb.com/#website" },
          breadcrumb: { "@id": "https://loradb.com/contact#breadcrumb" },
          mainEntity: {
            "@id": "https://loradb.com/#organization",
            "@type": "Organization",
            name: "LoraDB",
            url: "https://loradb.com",
            contactPoint: [
              {
                "@type": "ContactPoint",
                contactType: "customer support",
                email: "hello@loradb.com",
                url: "https://github.com/lora-db/lora/issues",
                availableLanguage: ["English"],
              },
              {
                "@type": "ContactPoint",
                contactType: "security",
                email: "security@loradb.com",
                availableLanguage: ["English"],
              },
            ],
          },
          inLanguage: "en",
        },
        {
          "@type": "BreadcrumbList",
          "@id": "https://loradb.com/contact#breadcrumb",
          itemListElement: [
            {
              "@type": "ListItem",
              position: 1,
              name: "Home",
              item: "https://loradb.com",
            },
            {
              "@type": "ListItem",
              position: 2,
              name: "Contact",
              item: "https://loradb.com/contact",
            },
          ],
        },
      ],
    }),
  }}
/>

# Contact the LoraDB Team

Building with LoraDB, hitting a wall, or weighing it up for a new
system? Pick the channel that matches what you need.

## Bugs and feature requests

Open an issue on GitHub. A small, reproducible repro — ideally the
Cypher that misbehaves plus the data model — goes a long way.

- **GitHub Issues** — [github.com/lora-db/lora/issues](https://github.com/lora-db/lora/issues)

## Discussion and help

Design questions, "is this the right tool for X", Cypher patterns,
integration notes.

- **Discord** — [discord.gg/loradb](https://discord.gg/vUgKb6C8Af)
- **GitHub Discussions** — [github.com/lora-db/lora/discussions](https://github.com/lora-db/lora/discussions)

## Updates

Release notes, deep-dives, and roadmap posts.

- **Blog** — [medium.com/loradb](https://medium.com/loradb)
- **X** — [@LoraDB](https://x.com/loradb)
- **LinkedIn** — [linkedin.com/company/loradb](https://linkedin.com/company/loradb)

## Business inquiries

Partnerships, integrations, support contracts, and anything that
doesn't fit the public channels.

- **Email** — [hello@loradb.com](mailto:hello@loradb.com)

## Security

Found a security issue? Please do **not** file a public issue. Use
GitHub's private vulnerability reporting from the repository Security
tab, or email [security@loradb.com](mailto:security@loradb.com). Include
the release tag or commit SHA, a short impact summary, and the smallest
query or HTTP request that reproduces the issue.

## See also

- [**What is LoraDB**](/docs) — introduction and who it's for.
- [**Why LoraDB**](/docs/why) — the longer-form case.
- [**Docs**](/docs) — installation, queries, and reference.
