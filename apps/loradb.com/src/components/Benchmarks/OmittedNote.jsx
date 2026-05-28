import React from "react";
import clsx from "clsx";

import { ENGINE_BY_ID } from "@site/src/lib/benchmarks";

import styles from "./OmittedNote.module.scss";

/**
 * Renders the "this workload couldn't be benched on engine X" notes
 * sourced from `report.md`. Groups the notes by workload so a workload
 * with multiple omissions reads as one block, not five.
 *
 * `notes` is a flat list of { workload, engine, body }. The grouping
 * happens here so callers can pass either a per-group slice or the
 * full per-competitor slice without having to pre-shape them.
 */
export default function OmittedNote({ notes, className }) {
  if (!notes || notes.length === 0) return null;

  const grouped = new Map();
  for (const n of notes) {
    if (!grouped.has(n.workload)) grouped.set(n.workload, []);
    grouped.get(n.workload).push(n);
  }

  return (
    <section
      className={clsx(styles.notes, className)}
      aria-label="Omitted workloads"
    >
      <header className={styles.header}>
        <span className={styles.eyebrow}>Omissions</span>
        <p className={styles.lede}>
          Workloads omitted only where the engine has no like-for-like
          equivalent — a missing scalar function, a different MERGE semantics, a
          reserved word. The reason is recorded with the workload, not silently
          dropped.
        </p>
      </header>
      <ul className={styles.list}>
        {Array.from(grouped.entries()).map(([workload, items]) => (
          <li key={workload} className={styles.item}>
            <div className={styles.itemHeader}>
              <code className={styles.workload}>{workload}</code>
              <div className={styles.engines}>
                {items.map((n) => (
                  <span key={n.engine} className={styles.engineTag}>
                    {ENGINE_BY_ID[n.engine]?.label ?? n.engine}
                  </span>
                ))}
              </div>
            </div>
            <ul className={styles.reasons}>
              {items.map((n) => (
                <li key={n.engine}>{n.body}</li>
              ))}
            </ul>
          </li>
        ))}
      </ul>
    </section>
  );
}
