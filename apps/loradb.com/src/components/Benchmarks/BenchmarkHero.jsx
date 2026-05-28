import React from "react";
import clsx from "clsx";

import styles from "./BenchmarkHero.module.scss";

/**
 * Hero shell for the benchmark routes. Two-column layout: a headline
 * on the left, a stats grid on the right. The page passes the stats
 * in as plain children so each page can decide what to surface
 * (overview pages → 4 cards; competitor pages → 3).
 *
 * Kept generic so it can also host the competitor pages without a
 * second hero variant.
 */
export default function BenchmarkHero({
  eyebrow,
  title,
  highlight,
  lede,
  meta,
  actions,
  stats,
}) {
  return (
    <section className={styles.hero}>
      <div className={styles.glow} aria-hidden="true" />
      <div className={styles.inner}>
        <div className={styles.copy}>
          {eyebrow ? (
            <p className={styles.eyebrow}>
              <span className={styles.dot} />
              {eyebrow}
            </p>
          ) : null}
          <h1 className={styles.title}>
            {title}
            {highlight ? (
              <>
                {" "}
                <span className={styles.titleAccent}>{highlight}</span>
              </>
            ) : null}
          </h1>
          {lede ? <p className={styles.lede}>{lede}</p> : null}
          {actions ? <div className={styles.actions}>{actions}</div> : null}
          {meta ? <div className={styles.meta}>{meta}</div> : null}
        </div>
        {stats ? <div className={styles.stats}>{stats}</div> : null}
      </div>
    </section>
  );
}

export function HeroStat({ value, label, hint, accent = false }) {
  return (
    <div className={clsx(styles.stat, accent && styles.statAccent)}>
      <div className={styles.statValue}>{value}</div>
      <div className={styles.statLabel}>{label}</div>
      {hint ? <div className={styles.statHint}>{hint}</div> : null}
    </div>
  );
}
