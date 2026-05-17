import type { ReactNode } from "react";
import type { Metadata } from "next";
import { ColorSchemeScript } from "@mantine/core";

import "@mantine/core/styles.css";
import "@mantine/notifications/styles.css";
import "@mantine/spotlight/styles.css";
import "@loradb/lora-query/styles.css";
import "@loradb/lora-graph-canvas/styles.css";
import "@glideapps/glide-data-grid/dist/index.css";
import "./globals.css";

import { Providers } from "./providers";

export const metadata: Metadata = {
  title: "LoraDB Playground",
  description:
    "In-browser IDE for the LoraDB graph database. Author Cypher, visualize the result as a graph or table, save snapshots, and share queries by URL.",
  icons: {
    icon: "/favicon.svg",
  },
  openGraph: {
    title: "LoraDB Playground",
    description: "In-browser Cypher IDE backed by the LoraDB WASM graph engine.",
    type: "website",
    images: [{ url: "/og-image.png", width: 1200, height: 630 }],
  },
  twitter: {
    card: "summary_large_image",
    title: "LoraDB Playground",
    description: "In-browser Cypher IDE for LoraDB.",
    images: ["/og-image.png"],
  },
};

export default function RootLayout({ children }: { children: ReactNode }) {
  return (
    <html lang="en" suppressHydrationWarning>
      <head>
        <ColorSchemeScript defaultColorScheme="dark" />
      </head>
      <body>
        <Providers>{children}</Providers>
      </body>
    </html>
  );
}
