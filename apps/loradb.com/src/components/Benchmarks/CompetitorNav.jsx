import React from "react";
import clsx from "clsx";
import Link from "@docusaurus/Link";

import { COMPETITORS } from "@site/src/lib/benchmarks";

import styles from "./CompetitorNav.module.scss";

/**
 * Tab strip linking the per-competitor pages. Rendered both on the
 * overview ("dive into a head-to-head") and on each competitor page
 * (switching between competitors without going back).
 *
 * The "Overview" pill is highlighted by passing `active="overview"`;
 * a specific engine id activates its tab.
 */
export default function CompetitorNav({ active, className }) {
  return (
    <nav className={clsx(styles.nav, className)} aria-label="Benchmark pages">
      <Link
        to="/benchmarks"
        className={clsx(
          styles.pill,
          active === "overview" && styles.pillActive,
        )}
        aria-current={active === "overview" ? "page" : undefined}
      >
        <span className={styles.pillDot} aria-hidden="true" />
        Overview
      </Link>
      {COMPETITORS.map((c) => (
        <Link
          key={c.id}
          to={`/benchmarks/lora-vs-${c.id}`}
          className={clsx(styles.pill, active === c.id && styles.pillActive)}
          aria-current={active === c.id ? "page" : undefined}
        >
          vs {c.label}
        </Link>
      ))}
    </nav>
  );
}
