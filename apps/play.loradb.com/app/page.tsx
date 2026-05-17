"use client";

import dynamic from "next/dynamic";

// The workbench is entirely client-side (Mantine + zustand + the
// editor / graph / grid surfaces all touch `window`). Skipping SSR
// avoids re-running it during the static prerender.
const Workbench = dynamic(
  () => import("./_components/Workbench").then((m) => m.Workbench),
  { ssr: false },
);

export default function HomePage() {
  return <Workbench />;
}
