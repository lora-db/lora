"use client";

import dynamic from "next/dynamic";

// The workbench is entirely client-side (Mantine + zustand + the
// editor / graph / grid surfaces all touch `window`). Skipping SSR
// avoids re-running it during the static prerender.
//
// The `loading` fallback mirrors the snapshot-restore overlay's
// visual rhythm (icon + primary label + muted subtitle on a dark
// backdrop) so the hydration window doesn't flash a blank page.
// Plain HTML / inline SVG because Mantine isn't hydrated yet.
const Workbench = dynamic(
  () => import("./_components/Workbench").then((m) => m.Workbench),
  {
    ssr: false,
    loading: () => <PlaygroundHydrationLoader />,
  },
);

function PlaygroundHydrationLoader() {
  return (
    <div className="playground-hydration" role="status" aria-live="polite">
      <div className="playground-hydration__card">
        <svg
          className="playground-hydration__icon"
          width="24"
          height="24"
          viewBox="0 0 24 24"
          fill="none"
          stroke="currentColor"
          strokeWidth="1.5"
          strokeLinecap="round"
          strokeLinejoin="round"
          aria-hidden="true"
        >
          <path d="M21 12a9 9 0 1 1-6.219-8.56" />
        </svg>
        <p className="playground-hydration__title">Loading the playground</p>
        <p className="playground-hydration__hint">Hang on a moment.</p>
      </div>
    </div>
  );
}

export default function HomePage() {
  return <Workbench />;
}
