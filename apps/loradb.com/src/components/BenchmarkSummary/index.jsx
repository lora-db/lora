import React, {useEffect, useMemo, useState} from 'react';
import useBaseUrl from '@docusaurus/useBaseUrl';

import styles from './styles.module.scss';

function formatNs(ns) {
  if (ns == null || Number.isNaN(Number(ns))) return '—';
  const value = Number(ns);
  if (value >= 1_000_000_000) return `${(value / 1_000_000_000).toFixed(2)} s`;
  if (value >= 1_000_000) return `${(value / 1_000_000).toFixed(2)} ms`;
  if (value >= 1_000) return `${(value / 1_000).toFixed(2)} µs`;
  return `${Math.round(value)} ns`;
}

function formatRatio(value) {
  if (value == null || Number.isNaN(Number(value))) return '—';
  return `${Number(value).toFixed(2)}x`;
}

function formatPct(value) {
  if (value == null || Number.isNaN(Number(value))) return '—';
  return `${Number(value).toFixed(1)}%`;
}

function formatDate(value) {
  if (!value) return 'unknown';
  const date = new Date(value);
  if (Number.isNaN(date.getTime())) return value;
  return date.toLocaleString(undefined, {
    year: 'numeric',
    month: 'short',
    day: 'numeric',
    hour: '2-digit',
    minute: '2-digit',
  });
}

function statusLabel(status) {
  switch (status) {
    case 'regressed':
      return 'regressed';
    case 'new':
      return 'new';
    case 'ok':
      return 'ok';
    default:
      return status || 'unknown';
  }
}

function statusClass(status) {
  switch (status) {
    case 'regressed':
      return styles.statusRegressed;
    case 'new':
      return styles.statusNew;
    case 'ok':
      return styles.statusOk;
    default:
      return styles.statusUnknown;
  }
}

export default function BenchmarkSummary({src = '/benchmarks/perf-smoke-summary.json'}) {
  const url = useBaseUrl(src);
  const [state, setState] = useState({status: 'loading', data: null, error: null});

  useEffect(() => {
    let cancelled = false;

    async function load() {
      try {
        const response = await fetch(url, {cache: 'no-cache'});
        if (!response.ok) {
          throw new Error(`${response.status} ${response.statusText}`);
        }
        const data = await response.json();
        if (!cancelled) setState({status: 'ready', data, error: null});
      } catch (error) {
        if (!cancelled) {
          setState({status: 'error', data: null, error: error.message});
        }
      }
    }

    load();
    return () => {
      cancelled = true;
    };
  }, [url]);

  const rows = useMemo(() => state.data?.benchmarks ?? [], [state.data]);
  const sortedRows = useMemo(
    () => [...rows].sort((a, b) => (b.baseline?.ratio ?? 0) - (a.baseline?.ratio ?? 0)),
    [rows],
  );
  const summary = state.data?.summary;
  const baseline = state.data?.baseline;
  const group = state.data?.groups?.[0];

  if (state.status === 'loading') {
    return (
      <div className={styles.notice} role="status">
        Loading benchmark summary from <code>{src}</code>…
      </div>
    );
  }

  if (state.status === 'error') {
    return (
      <div className={styles.notice} role="alert">
        Could not load <code>{src}</code>: {state.error}
      </div>
    );
  }

  return (
    <section className={styles.report} aria-label="Benchmark summary loaded from JSON">
      <div className={styles.meta}>
        <div>
          <span className={styles.metaLabel}>Suite</span>
          <strong>{state.data.suite}</strong>
        </div>
        <div>
          <span className={styles.metaLabel}>Generated</span>
          <strong>{formatDate(state.data.generated_at)}</strong>
        </div>
        <div>
          <span className={styles.metaLabel}>Source</span>
          <code>{src}</code>
        </div>
      </div>

      <div className={styles.metrics}>
        <div>
          <span className={styles.metricValue}>{summary?.benchmark_count ?? rows.length}</span>
          <span className={styles.metricLabel}>benchmarks</span>
        </div>
        <div>
          <span className={styles.metricValue}>{baseline?.regressed_count ?? 0}</span>
          <span className={styles.metricLabel}>regressions</span>
        </div>
        <div>
          <span className={styles.metricValue}>{formatNs(group?.median_ns_per_iter)}</span>
          <span className={styles.metricLabel}>median</span>
        </div>
        <div>
          <span className={styles.metricValue}>{formatNs(group?.p95_ns_per_iter)}</span>
          <span className={styles.metricLabel}>p95</span>
        </div>
      </div>

      <div className="table-wrapper">
        <table className={styles.table}>
          <thead>
            <tr>
              <th>Benchmark</th>
              <th>Current</th>
              <th>Baseline</th>
              <th>Ratio</th>
              <th>Error</th>
              <th>Status</th>
            </tr>
          </thead>
          <tbody>
            {sortedRows.map((bench) => (
              <tr key={bench.name}>
                <td>
                  <code>{bench.name}</code>
                </td>
                <td>{formatNs(bench.ns_per_iter)}</td>
                <td>{formatNs(bench.baseline?.ns_per_iter)}</td>
                <td>{formatRatio(bench.baseline?.ratio)}</td>
                <td>{formatPct(bench.relative_error_pct)}</td>
                <td>
                  <span className={`${styles.status} ${statusClass(bench.baseline?.status)}`}>
                    {statusLabel(bench.baseline?.status)}
                  </span>
                </td>
              </tr>
            ))}
          </tbody>
        </table>
      </div>

      <p className={styles.caption}>
        Sorted by baseline ratio, highest first. The smoke gate currently allows
        up to {formatRatio(baseline?.default_threshold)} before failing CI.
      </p>
    </section>
  );
}
