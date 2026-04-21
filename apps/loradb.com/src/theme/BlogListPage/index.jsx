// Swizzled `BlogListPage` that frames Docusaurus's default blog index with a
// LoraDB-branded hero. The rest of the page (post list, paginator, sidebar)
// is delegated to the stock components so we don't diverge from Docusaurus
// semantics (structured data, SEO, tag navigation).
import React from 'react';
import clsx from 'clsx';
import useDocusaurusContext from '@docusaurus/useDocusaurusContext';
import Link from '@docusaurus/Link';
import {
  PageMetadata,
  HtmlClassNameProvider,
  ThemeClassNames,
} from '@docusaurus/theme-common';
import BlogLayout from '@theme/BlogLayout';
import BlogListPaginator from '@theme/BlogListPaginator';
import SearchMetadata from '@theme/SearchMetadata';
import BlogPostItems from '@theme/BlogPostItems';
import BlogListPageStructuredData from '@theme/BlogListPage/StructuredData';

function BlogListPageMetadata(props) {
  const { metadata } = props;
  const {
    siteConfig: { title: siteTitle },
  } = useDocusaurusContext();
  const { blogDescription, blogTitle, permalink } = metadata;
  const isBlogOnlyMode = permalink === '/';
  const title = isBlogOnlyMode ? siteTitle : blogTitle;
  return (
    <>
      <PageMetadata title={title} description={blogDescription} />
      <SearchMetadata tag="blog_posts_list" />
    </>
  );
}

function BlogHero({ title, description }) {
  return (
    <header className="blog-hero">
      <div className="blog-hero__glow" aria-hidden="true" />
      <div className="blog-hero__inner">
        <p className="blog-hero__eyebrow">
          <span className="blog-hero__dot" />
          Engineering journal
        </p>
        <h1 className="blog-hero__title">{title}</h1>
        {description && (
          <p className="blog-hero__description">{description}</p>
        )}
        <div className="blog-hero__actions">
          {/*
            Feed URLs are static XML files emitted by the blog plugin at
            build time — not SPA routes. Using <a> instead of <Link>
            avoids Docusaurus' broken-link checker (which only sees SPA
            routes) and gives browsers a real download target.
          */}
          <a href="/blog/rss.xml" className="blog-hero__feed">
            <svg
              width="14"
              height="14"
              viewBox="0 0 24 24"
              fill="none"
              stroke="currentColor"
              strokeWidth="2"
              strokeLinecap="round"
              strokeLinejoin="round"
              aria-hidden="true"
            >
              <path d="M4 11a9 9 0 0 1 9 9" />
              <path d="M4 4a16 16 0 0 1 16 16" />
              <circle cx="5" cy="19" r="1" />
            </svg>
            RSS
          </a>
          <a href="/blog/atom.xml" className="blog-hero__feed">
            Atom
          </a>
          <Link to="/docs" className="blog-hero__feed blog-hero__feed--muted">
            Read the docs
            <svg
              width="12"
              height="12"
              viewBox="0 0 24 24"
              fill="none"
              stroke="currentColor"
              strokeWidth="2.2"
              strokeLinecap="round"
              strokeLinejoin="round"
              aria-hidden="true"
            >
              <path d="M5 12h14M13 5l7 7-7 7" />
            </svg>
          </Link>
        </div>
      </div>
    </header>
  );
}

function BlogListPageContent(props) {
  const { metadata, items, sidebar } = props;
  const { blogTitle, blogDescription } = metadata;
  return (
    <BlogLayout sidebar={sidebar}>
      <BlogHero title={blogTitle} description={blogDescription} />
      <BlogPostItems items={items} />
      <BlogListPaginator metadata={metadata} />
    </BlogLayout>
  );
}

export default function BlogListPage(props) {
  return (
    <HtmlClassNameProvider
      className={clsx(
        ThemeClassNames.wrapper.blogPages,
        ThemeClassNames.page.blogListPage,
        'blog-list-page--loradb',
      )}
    >
      <BlogListPageMetadata {...props} />
      <BlogListPageStructuredData {...props} />
      <BlogListPageContent {...props} />
    </HtmlClassNameProvider>
  );
}
