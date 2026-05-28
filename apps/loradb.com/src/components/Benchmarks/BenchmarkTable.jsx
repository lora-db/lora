import React from "react";
import clsx from "clsx";

import {
  ENGINE_BY_ID,
  formatRatio,
  resolveCell,
} from "@site/src/lib/benchmarks";
import WinnerBadge from "./WinnerBadge";

import styles from "./BenchmarkTable.module.scss";

/**
 * The shared compare-engines table. Used by:
 *   - the overview group summary (`rows` = per-group geomean slowdowns)
 *   - per-group workload tables (`rows` = per-workload raw timings)
 *   - per-competitor comparison tables (`engines` shortened to two)
 *
 * Each cell is one of: { kind: 'winner' }, { kind: 'ratio', value }, or
 * { kind: 'omitted' }. Resolution lives in `lib/benchmarks` so the
 * table itself only worries about layout.
 *
 * Props:
 *   - engines: ordered list of engine descriptors to render columns for
 *   - rows: [{ key, label, sub?, results: { [engineId]: value|object } }]
 *           `value` is either the literal "fastest", a number (ratio
 *           slowdown), null (omitted), or { time, slowdown }.
 *   - winnerKey: which row key to highlight (typically "total" on the
 *           overview)
 *   - compact: tighten paddings — used on per-competitor pages
 */
export default function BenchmarkTable({
  engines,
  rows,
  winnerKey,
  compact = false,
  caption,
}) {
  return (
    <div className={styles.wrap}>
      <div
        className={clsx(styles.scroll, compact && styles.scrollCompact)}
        role="region"
        aria-label={caption ?? "Benchmark comparison"}
      >
        <table className={styles.table}>
          {caption ? (
            <caption className={styles.caption}>{caption}</caption>
          ) : null}
          <thead>
            <tr>
              <th scope="col" className={styles.firstHeader}>
                <span className={styles.headerInner}>Workload</span>
              </th>
              <th scope="col" className={styles.numericHeader}>
                <span className={styles.headerInner}>Size</span>
              </th>
              {engines.map((engine) => (
                <th
                  scope="col"
                  key={engine.id}
                  className={clsx(
                    styles.engineHeader,
                    engine.id === "lora" && styles.engineHeaderLora,
                  )}
                >
                  <span className={styles.headerInner}>{engine.label}</span>
                </th>
              ))}
              <th scope="col" className={styles.winnerHeader}>
                <span className={styles.headerInner}>Winner</span>
              </th>
            </tr>
          </thead>
          <tbody>
            {rows.map((row) => {
              const isTotal = row.key === winnerKey;
              return (
                <tr key={row.key} className={clsx(isTotal && styles.totalRow)}>
                  <th scope="row" className={styles.firstCell}>
                    <span className={styles.rowLabel}>{row.label}</span>
                    {row.sub ? (
                      <span className={styles.rowSub}>{row.sub}</span>
                    ) : null}
                  </th>
                  <td className={styles.sizeCell}>
                    {row.size != null ? row.size : "—"}
                  </td>
                  {engines.map((engine) => {
                    const raw = row.results?.[engine.id];
                    return (
                      <Cell
                        key={engine.id}
                        raw={raw}
                        engine={engine}
                        isOverallWinner={row.winner === engine.id}
                        isLora={engine.id === "lora"}
                      />
                    );
                  })}
                  <td className={styles.winnerCell}>
                    {row.winner ? (
                      <WinnerEngineLabel engineId={row.winner} />
                    ) : (
                      <span className={styles.omittedText}>—</span>
                    )}
                  </td>
                </tr>
              );
            })}
          </tbody>
        </table>
      </div>
    </div>
  );
}

function WinnerEngineLabel({ engineId }) {
  const engine = ENGINE_BY_ID[engineId];
  if (!engine) return engineId;
  return (
    <span
      className={clsx(
        styles.winnerEngine,
        engineId === "lora" && styles.winnerEngineLora,
      )}
    >
      <span className={styles.winnerDot} aria-hidden="true" />
      {engine.label}
    </span>
  );
}

function Cell({ raw, engine, isOverallWinner, isLora }) {
  // Two shapes: { time, slowdown } (workload rows) or a single value
  // (group summary rows — "fastest" | number | null).
  let display;
  let kind;
  let time = null;

  if (raw && typeof raw === "object" && "time" in raw) {
    time = raw.time;
    if (raw.time == null && raw.slowdown == null) {
      kind = "omitted";
    } else if (raw.slowdown == null) {
      kind = "winner";
    } else {
      kind = "ratio";
      display = `${formatRatio(raw.slowdown)} slower`;
    }
  } else {
    const resolved = resolveCell(raw);
    kind = resolved.kind;
    if (kind === "ratio") display = `${formatRatio(resolved.value)} slower`;
  }

  if (kind === "omitted") {
    return (
      <td
        className={clsx(
          styles.cell,
          styles.cellOmitted,
          isLora && styles.cellLora,
        )}
      >
        <span className={styles.omittedText}>omitted</span>
      </td>
    );
  }

  if (kind === "winner") {
    return (
      <td
        className={clsx(
          styles.cell,
          styles.cellWinner,
          isLora && styles.cellLora,
          isOverallWinner && styles.cellOverallWinner,
        )}
      >
        {time ? <span className={styles.cellTime}>{time}</span> : null}
        <WinnerBadge engine={engine.id} variant={time ? "soft" : "solid"}>
          fastest
        </WinnerBadge>
      </td>
    );
  }

  return (
    <td className={clsx(styles.cell, isLora && styles.cellLora)}>
      {time ? <span className={styles.cellTime}>{time}</span> : null}
      <span className={styles.cellRatio}>{display}</span>
    </td>
  );
}
