import React from "react";
import clsx from "clsx";
import Link from "@docusaurus/Link";

import {
  ENGINE_BY_ID,
  formatRatio,
  resolveCell,
} from "@site/src/lib/benchmarks";

import styles from "./GroupCard.module.scss";

/**
 * Group summary card for the overview page. Shows the group name,
 * workload count, group winner, and the per-engine geomean slowdowns
 * as compact chips so a reader can scan the whole suite at a glance
 * without needing the wide summary table.
 */
export default function GroupCard({ group, engines, anchorHref }) {
  const summary = group.summary;
  const winnerEngine = ENGINE_BY_ID[group.winner];

  const competitorChips = engines
    .filter((e) => e.id !== "lora")
    .map((engine) => {
      const cell = resolveCell(summary[engine.id]);
      return {
        engine,
        cell,
        isGroupWinner: group.winner === engine.id,
      };
    });

  const loraCell = resolveCell(summary.lora);

  return (
    <article
      className={clsx(styles.card, group.winner === "lora" && styles.cardWin)}
    >
      <header className={styles.header}>
        <div>
          <div className={styles.name}>
            {anchorHref ? (
              <Link to={anchorHref}>{group.name}</Link>
            ) : (
              group.name
            )}
          </div>
          <div className={styles.count}>
            {group.workloadCount}{" "}
            {group.workloadCount === 1 ? "workload" : "workloads"}
          </div>
        </div>
        <div className={styles.winner}>
          <span className={styles.winnerLabel}>Winner</span>
          <span
            className={clsx(
              styles.winnerValue,
              group.winner === "lora" && styles.winnerValueLora,
            )}
          >
            {winnerEngine?.label ?? group.winner}
          </span>
        </div>
      </header>

      <div className={styles.lora}>
        <span className={styles.loraLabel}>LoraDB</span>
        <span
          className={clsx(
            styles.loraValue,
            loraCell.kind === "winner" && styles.loraValueWinner,
          )}
        >
          {loraCell.kind === "winner"
            ? "fastest"
            : loraCell.kind === "ratio"
              ? `${formatRatio(loraCell.value)} slower`
              : "—"}
        </span>
      </div>

      <ul className={styles.chips}>
        {competitorChips.map(({ engine, cell, isGroupWinner }) => (
          <li
            key={engine.id}
            className={clsx(
              styles.chip,
              styles[`chip_${cell.kind}`],
              isGroupWinner && styles.chipWinner,
            )}
          >
            <span className={styles.chipEngine}>{engine.short}</span>
            <span className={styles.chipValue}>
              {cell.kind === "winner"
                ? "fastest"
                : cell.kind === "ratio"
                  ? formatRatio(cell.value)
                  : "n/a"}
            </span>
          </li>
        ))}
      </ul>
    </article>
  );
}
