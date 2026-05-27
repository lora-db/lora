// Wrap-swizzle of `theme/Blog/Pages/BlogAuthorsPostsPage` to inject a
// Person JSON-LD block per author page. The default component already
// renders `author.description` as a visible bio and lists the
// author's posts; this wrapper adds the schema.org structured data
// so the per-author pages are properly attached to the LoraDB
// Organization in the knowledge graph.
//
// `author.socials` is normalised here into a `sameAs` array. The
// defaults shown to the layout (`name`, `url`, `imageURL`) come from
// blog/authors.yml — we read them off the prop instead of duplicating.
import React from "react";
import Head from "@docusaurus/Head";
import { useLocation } from "@docusaurus/router";
import BlogAuthorsPostsPage from "@theme-original/Blog/Pages/BlogAuthorsPostsPage";

const SITE_URL = "https://loradb.com";

function socialsToSameAs(socials) {
  if (!socials) return [];
  const out = [];
  const push = (url) => {
    if (url && !out.includes(url)) out.push(url);
  };
  // Each value can be either a bare handle or a full URL — accept both.
  const ABS = /^https?:\/\//i;
  const handle = (v) => (typeof v === "string" ? v : v?.value);
  const map = {
    github: (h) => `https://github.com/${h}`,
    x: (h) => `https://x.com/${h}`,
    twitter: (h) => `https://x.com/${h}`,
    linkedin: (h) =>
      h.startsWith("in/") || h.startsWith("company/")
        ? `https://linkedin.com/${h}`
        : `https://linkedin.com/in/${h}`,
    mastodon: (h) => (h.startsWith("@") ? null : `https://${h}`),
    bluesky: (h) => `https://bsky.app/profile/${h}`,
  };
  for (const [k, raw] of Object.entries(socials)) {
    const v = handle(raw);
    if (!v) continue;
    if (ABS.test(v)) {
      push(v);
      continue;
    }
    const fn = map[k.toLowerCase()];
    if (fn) {
      const u = fn(v);
      if (u) push(u);
    }
  }
  return out;
}

function absolutise(maybeRel) {
  if (!maybeRel) return undefined;
  if (/^https?:\/\//i.test(maybeRel)) return maybeRel;
  return `${SITE_URL}${maybeRel.startsWith("/") ? "" : "/"}${maybeRel}`;
}

export default function BlogAuthorsPostsPageWrapper(props) {
  const { author } = props;
  const { pathname } = useLocation();
  // Strip trailing slash for canonical comparison; restore one in
  // the absolute URL so it matches the sitemap entry exactly.
  const cleanPath = pathname.replace(/\/+$/, "");
  const pageUrl = `${SITE_URL}${cleanPath}`;

  const personLd = author && {
    "@context": "https://schema.org",
    "@type": "Person",
    "@id": `${pageUrl}#person`,
    name: author.name,
    url: pageUrl,
    description: author.description,
    image: absolutise(author.imageURL),
    jobTitle: author.title,
    sameAs: socialsToSameAs(author.socials),
    worksFor: {
      "@id": `${SITE_URL}/#organization`,
    },
  };

  return (
    <>
      {personLd && (
        <Head>
          <script type="application/ld+json">{JSON.stringify(personLd)}</script>
        </Head>
      )}
      <BlogAuthorsPostsPage {...props} />
    </>
  );
}
