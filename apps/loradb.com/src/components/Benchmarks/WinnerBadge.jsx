import React from "react";
import clsx from "clsx";

import styles from "./WinnerBadge.module.scss";

/**
 * Tiny pill that marks the fastest result in a row, or the winner of
 * a group/competitor card. Variants are visual only — same component,
 * different background gradient strength.
 *   - `solid` (default) – filled gradient, used in headlines.
 *   - `soft` – muted tint, used in table cells next to "fastest".
 */
export default function WinnerBadge({
  engine,
  variant = "solid",
  children,
  className,
}) {
  return (
    <span
      className={clsx(
        styles.badge,
        variant === "soft" && styles.badgeSoft,
        engine === "lora" && styles.badgeLora,
        className,
      )}
    >
      <span className={styles.dot} aria-hidden="true" />
      {children ?? "fastest"}
    </span>
  );
}
