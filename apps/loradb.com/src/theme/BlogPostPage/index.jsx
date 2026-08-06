// Swizzle wrapper around theme-classic's BlogPostPage.
//
// Docusaurus emits BlogPosting JSON-LD for blog posts and BreadcrumbList
// for doc pages, but never BreadcrumbList for a blog post. Docs and the
// benchmark comparison pages both carry one, so blog posts were the only
// indexable page type on the site without breadcrumb structured data.
//
// BreadcrumbList is what lets Google render the "loradb.com › Blog ›
// <post>" trail in place of a raw URL in search results. It also states
// the Home → Blog → post hierarchy explicitly rather than leaving Google
// to infer it from URL shape.
//
// The trail is built from the post's own metadata, and the permalink is
// normalised to drop any trailing slash so it matches the page's
// <link rel="canonical"> (the site runs trailingSlash: false).
//
// This wraps rather than replaces the original component, so Docusaurus
// upgrades to BlogPostPage internals continue to flow through.

import React from "react";
import Head from "@docusaurus/Head";
import useDocusaurusContext from "@docusaurus/useDocusaurusContext";
import OriginalBlogPostPage from "@theme-original/BlogPostPage";

export default function BlogPostPageWrapper(props) {
  const { siteConfig } = useDocusaurusContext();
  const siteUrl = siteConfig.url.replace(/\/$/, "");

  const metadata = props?.content?.metadata;
  const permalink = (metadata?.permalink || "").replace(/\/+$/, "");
  const title = metadata?.title;

  // Bail out rather than emit a half-populated breadcrumb: malformed
  // structured data is worse than none, since Google reports it as an
  // error against the page.
  const schema =
    permalink && title
      ? {
          "@context": "https://schema.org",
          "@type": "BreadcrumbList",
          itemListElement: [
            {
              "@type": "ListItem",
              position: 1,
              name: "Home",
              item: `${siteUrl}/`,
            },
            {
              "@type": "ListItem",
              position: 2,
              name: "Blog",
              item: `${siteUrl}/blog`,
            },
            {
              "@type": "ListItem",
              position: 3,
              name: title,
              item: `${siteUrl}${permalink}`,
            },
          ],
        }
      : null;

  return (
    <>
      {schema && (
        <Head>
          <script type="application/ld+json">{JSON.stringify(schema)}</script>
        </Head>
      )}
      <OriginalBlogPostPage {...props} />
    </>
  );
}
