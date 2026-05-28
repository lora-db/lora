import React from "react";
import clsx from "clsx";

import { formatRatio } from "@site/src/lib/benchmarks";

import styles from "./SlowdownBar.module.scss";

/**
 * Horizontal bar that maps a slowdown ratio onto a logarithmic scale.
 *
 * Why log scale: total-row slowdowns span 2.3× → 57.2× in this suite,
 * and the per-row maxima can hit thousands. A linear bar would crush
 * everything under 10× into pixel dust. log10 keeps both ends
 * visible — a 2× bar fills ~30%, a 10× bar ~60%, a 100× bar ~85%.
 */
export default function SlowdownBar({
  value,
  engine,
  label,
  max = 1000,
  className,
}) {
  const isWinner = value === "fastest";
  const numeric = typeof value === "number" ? value : null;
  const isOmitted = !isWinner && numeric == null;

  let pct = 0;
  if (isWinner) pct = 4;
  else if (numeric != null) {
    const safe = Math.max(1, numeric);
    const denom = Math.log10(max);
    pct = Math.min(100, Math.max(4, (Math.log10(safe) / denom) * 100));
  }

  const display = isWinner
    ? "fastest"
    : isOmitted
      ? "n/a"
      : `${formatRatio(numeric)} slower`;

  return (
    <div
      className={clsx(
        styles.row,
        isWinner && styles.winner,
        engine === "lora" && styles.lora,
        isOmitted && styles.omitted,
        className,
      )}
      role="img"
      aria-label={`${label ?? engine}: ${display}`}
    >
      {label ? <span className={styles.label}>{label}</span> : null}
      <span className={styles.track}>
        <span className={styles.fill} style={{ width: `${pct}%` }} />
      </span>
      <span className={styles.value}>{display}</span>
    </div>
  );
}
