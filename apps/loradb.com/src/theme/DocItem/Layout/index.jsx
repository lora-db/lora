// Swizzle wrapper around theme-classic's DocItem/Layout.
//
// Out of the box Docusaurus emits only BreadcrumbList JSON-LD on
// individual doc pages — no Article schema, even though docs are
// classic Article shapes (a headline, a body, an author, a publisher).
//
// We wrap the original layout with a <Head> injection that adds a
// TechArticle JSON-LD blob per doc page. TechArticle is the more
// specific schema.org type for instructional / reference content; it
// inherits from Article and is preferred by Google's structured-data
// extractor for developer-facing documentation.
//
// Fields are derived from the doc's frontmatter (title, description)
// and the site's URL configuration. Author and publisher reference the
// global Organization @id from docusaurus.config.js by URL fragment,
// so the doc-level JSON-LD links into the same entity graph the
// homepage emits.

import React from "react";
import Head from "@docusaurus/Head";
import useDocusaurusContext from "@docusaurus/useDocusaurusContext";
import { useDoc } from "@docusaurus/plugin-content-docs/client";
import OriginalLayout from "@theme-original/DocItem/Layout";

export default function LayoutWrapper(props) {
  const { metadata, frontMatter } = useDoc();
  const { siteConfig } = useDocusaurusContext();

  const siteUrl = siteConfig.url.replace(/\/$/, "");
  // Index docs (docs/index.md, docs/queries/index.md) carry a permalink
  // with a trailing slash: "/docs/", "/docs/queries/". The site runs
  // trailingSlash: false, so those forms 308-redirect and disagree with
  // the <link rel="canonical"> emitted on the very same page. Feeding the
  // raw permalink into url / mainEntityOfPage pointed the structured data
  // at a redirect and split the page's identity across two URLs.
  const permalink = metadata.permalink.replace(/\/+$/, "") || "/";
  const url = `${siteUrl}${permalink}`;

  const schema = {
    "@context": "https://schema.org",
    "@type": "TechArticle",
    headline: frontMatter.title || metadata.title,
    name: frontMatter.title || metadata.title,
    description: frontMatter.description || metadata.description || undefined,
    url,
    mainEntityOfPage: url,
    inLanguage: "en",
    isPartOf: { "@id": `${siteUrl}/#software` },
    author: { "@id": `${siteUrl}/#organization` },
    publisher: { "@id": `${siteUrl}/#organization` },
    proficiencyLevel: "Beginner",
  };
  if (frontMatter.keywords && Array.isArray(frontMatter.keywords)) {
    schema.keywords = frontMatter.keywords.join(", ");
  }
  if (metadata.lastUpdatedAt) {
    schema.dateModified = new Date(metadata.lastUpdatedAt * 1000).toISOString();
  }

  return (
    <>
      <Head>
        <script type="application/ld+json">{JSON.stringify(schema)}</script>
      </Head>
      <OriginalLayout {...props} />
    </>
  );
}
