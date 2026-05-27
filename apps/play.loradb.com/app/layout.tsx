import type { ReactNode } from "react";
import type { Metadata, Viewport } from "next";
import { ColorSchemeScript } from "@mantine/core";

import "@mantine/core/styles.css";
import "@mantine/notifications/styles.css";
import "@mantine/spotlight/styles.css";
import "@loradb/lora-query/styles.css";
import "@loradb/lora-graph-canvas/styles.css";
import "@glideapps/glide-data-grid/dist/index.css";
import "./globals.css";

import { Providers } from "./providers";

const SITE_URL = "https://play.loradb.com";
const MARKETING_URL = "https://loradb.com";
const TITLE =
  "LoraDB Playground — in-browser Cypher IDE for the LoraDB graph database";
const DESCRIPTION =
  "Try LoraDB without installing anything. The Playground is an in-browser IDE for the LoraDB graph database — author Cypher, visualize results as a graph or table, save snapshots, and share queries by URL. Runs entirely in your browser via WebAssembly.";
const SHORT_DESCRIPTION =
  "In-browser Cypher IDE for the LoraDB graph database. Runs entirely in your browser via WebAssembly.";

export const metadata: Metadata = {
  metadataBase: new URL(SITE_URL),
  title: TITLE,
  description: DESCRIPTION,
  applicationName: "LoraDB Playground",
  keywords: [
    "LoraDB",
    "Cypher",
    "Cypher playground",
    "graph database",
    "graph database playground",
    "WebAssembly database",
    "in-browser Cypher IDE",
    "knowledge graph",
    "property graph",
    "embedded graph database",
  ],
  authors: [{ name: "LoraDB", url: MARKETING_URL }],
  creator: "LoraDB",
  publisher: "LoraDB",
  alternates: {
    canonical: "/",
  },
  robots: {
    index: true,
    follow: true,
    googleBot: {
      index: true,
      follow: true,
      "max-image-preview": "large",
      "max-snippet": -1,
    },
  },
  icons: {
    // Mirrors the loradb.com favicon set so both surfaces share one mark.
    // Ordered most-preferred first; browsers pick SVG, fall back to .ico.
    icon: [
      { url: "/favicon.svg", type: "image/svg+xml" },
      { url: "/favicon.ico", sizes: "any" },
      { url: "/favicon-16x16.png", type: "image/png", sizes: "16x16" },
      { url: "/favicon-32x32.png", type: "image/png", sizes: "32x32" },
      { url: "/favicon-48x48.png", type: "image/png", sizes: "48x48" },
      { url: "/favicon-96x96.png", type: "image/png", sizes: "96x96" },
    ],
    apple: "/apple-touch-icon.png",
  },
  openGraph: {
    title: TITLE,
    description: SHORT_DESCRIPTION,
    type: "website",
    url: SITE_URL,
    siteName: "LoraDB Playground",
    locale: "en_US",
    images: [
      {
        url: "/og-image.png",
        width: 1200,
        height: 630,
        alt: "LoraDB Playground — in-browser Cypher IDE.",
      },
    ],
  },
  twitter: {
    card: "summary_large_image",
    title: TITLE,
    description: SHORT_DESCRIPTION,
    site: "@loradb",
    creator: "@loradb",
    images: ["/og-image.png"],
  },
};

export const viewport: Viewport = {
  themeColor: "#0a0f1f",
  width: "device-width",
  initialScale: 1,
};

// JSON-LD. Two facts about this surface that crawlers can rely on:
//   - It is a free, browser-based developer tool ("WebApplication" plus
//     SoftwareApplication.DeveloperApplication) backed by the LoraDB
//     engine.
//   - It is published by the same Organization that owns loradb.com,
//     so the two surfaces consolidate as one publisher in the
//     knowledge graph.
// No reviews/ratings, no fake prices — just truthful product
// positioning and a pointer back to the marketing site.
const STRUCTURED_DATA = {
  "@context": "https://schema.org",
  "@graph": [
    {
      "@type": "Organization",
      "@id": `${MARKETING_URL}/#organization`,
      name: "LoraDB",
      url: MARKETING_URL,
      logo: `${MARKETING_URL}/img/meta/icon-512.png`,
      sameAs: [
        "https://github.com/lora-db/lora",
        "https://x.com/loradb",
        "https://discord.gg/vUgKb6C8Af",
        "https://linkedin.com/company/loradb",
        "https://medium.com/loradb",
      ],
    },
    {
      "@type": "WebSite",
      "@id": `${SITE_URL}/#website`,
      url: SITE_URL,
      name: "LoraDB Playground",
      description: SHORT_DESCRIPTION,
      publisher: { "@id": `${MARKETING_URL}/#organization` },
      inLanguage: "en",
    },
    {
      "@type": ["WebApplication", "SoftwareApplication"],
      "@id": `${SITE_URL}/#webapp`,
      name: "LoraDB Playground",
      url: SITE_URL,
      applicationCategory: "DeveloperApplication",
      applicationSubCategory: "DatabaseApplication",
      operatingSystem: "Any (modern web browser with WebAssembly support)",
      browserRequirements: "Requires JavaScript, WebAssembly, and IndexedDB.",
      description: DESCRIPTION,
      featureList: [
        "Cypher query editor with syntax highlighting and autocomplete",
        "2D canvas and WebGL graph visualization",
        "Tabular result grid",
        "Saved queries and snapshots persisted in IndexedDB",
        "Shareable queries via URL",
        "Runs entirely client-side via WebAssembly — no server required",
      ],
      offers: {
        "@type": "Offer",
        price: "0",
        priceCurrency: "USD",
      },
      isPartOf: { "@id": `${MARKETING_URL}/#software` },
      publisher: { "@id": `${MARKETING_URL}/#organization` },
      author: { "@id": `${MARKETING_URL}/#organization` },
    },
  ],
};

export default function RootLayout({ children }: { children: ReactNode }) {
  return (
    <html lang="en" suppressHydrationWarning>
      <head>
        <ColorSchemeScript defaultColorScheme="dark" />
        <script
          type="application/ld+json"
          // JSON.stringify keeps payload safe — no user data is interpolated.
          dangerouslySetInnerHTML={{ __html: JSON.stringify(STRUCTURED_DATA) }}
        />
      </head>
      <body>
        <Providers>{children}</Providers>
        {/*
          Static fallback for users with JavaScript disabled. The Playground
          is a fully client-side WebAssembly app, so the prerendered body is
          otherwise empty.
        */}
        <noscript>
          <div className="playground-noscript">
            <h1>LoraDB Playground</h1>
            <p>
              The LoraDB Playground is an in-browser IDE for the{" "}
              <a href="https://loradb.com">LoraDB graph database</a>. Author
              Cypher, visualize results as a graph or table, save snapshots, and
              share queries by URL. It runs entirely in your browser via
              WebAssembly — no server, no sign-up.
            </p>
            <p>
              The Playground needs JavaScript and WebAssembly to run. Enable
              both and reload, or read the{" "}
              <a href="https://loradb.com/docs">LoraDB documentation</a> and
              install a local binding instead.
            </p>
          </div>
        </noscript>
      </body>
    </html>
  );
}
