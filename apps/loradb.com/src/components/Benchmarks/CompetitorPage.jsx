import React from "react";
import clsx from "clsx";
import Layout from "@theme/Layout";
import Head from "@docusaurus/Head";
import Link from "@docusaurus/Link";

import BenchmarkHero from "@site/src/components/Benchmarks/BenchmarkHero";
import BenchmarkTable from "@site/src/components/Benchmarks/BenchmarkTable";
import CompetitorNav from "@site/src/components/Benchmarks/CompetitorNav";
import OmittedNote from "@site/src/components/Benchmarks/OmittedNote";
import SlowdownBar from "@site/src/components/Benchmarks/SlowdownBar";

import {
  COMPETITOR_TAGLINE,
  ENGINE_BY_ID,
  GROUPS,
  TOTAL_ROW,
  WORKLOADS,
  competitorBreakdown,
  formatRatio,
  notesForCompetitor,
} from "@site/src/lib/benchmarks";
import useNavbarHide from "@site/src/lib/useNavbarHide";

import styles from "@site/src/pages/benchmarks/benchmarks.module.scss";

const SITE_URL = "https://loradb.com";

/**
 * Renders the LoraDB-vs-X comparison page.
 *
 * One implementation reused by every per-competitor route so the
 * /benchmarks/lora-vs-{kuzu,grafeo,...} pages stay in sync as the data is
 * refreshed.
 */
export default function CompetitorPage({ engineId }) {
  const [navRef, navHidden] = useNavbarHide();
  const engine = ENGINE_BY_ID[engineId];
  if (!engine) return null;

  const tagline = COMPETITOR_TAGLINE[engineId];
  const lora = ENGINE_BY_ID.lora;
  const breakdown = competitorBreakdown(engineId);
  const omitted = notesForCompetitor(engineId);
  const totalSlowdown = TOTAL_ROW.summary[engineId];
  const totalSlowdownDisplay =
    totalSlowdown === "fastest" || totalSlowdown == null
      ? "—"
      : `${formatRatio(totalSlowdown)}`;

  const headline = ((slowdown) => {
    if (typeof slowdown !== "number") return "Head-to-head benchmark.";
    if (slowdown >= 10)
      return `LoraDB is ${formatRatio(slowdown)} faster overall.`;
    if (slowdown >= 2)
      return `LoraDB leads by ${formatRatio(slowdown)} overall.`;
    return `LoraDB is faster on this suite.`;
  })(totalSlowdown);

  const structuredData = {
    "@context": "https://schema.org",
    "@graph": [
      {
        "@type": "WebPage",
        "@id": `${SITE_URL}/benchmarks/lora-vs-${engineId}#webpage`,
        url: `${SITE_URL}/benchmarks/lora-vs-${engineId}`,
        name: `LoraDB vs ${engine.label} — benchmark comparison`,
        description: `Head-to-head benchmark of LoraDB against ${engine.label} across the comparisons/ suite.`,
        isPartOf: { "@id": `${SITE_URL}/#website` },
        mainEntity: { "@id": `${SITE_URL}/#software` },
        inLanguage: "en",
      },
      {
        "@type": "BreadcrumbList",
        itemListElement: [
          {
            "@type": "ListItem",
            position: 1,
            name: "Home",
            item: SITE_URL,
          },
          {
            "@type": "ListItem",
            position: 2,
            name: "Benchmarks",
            item: `${SITE_URL}/benchmarks`,
          },
          {
            "@type": "ListItem",
            position: 3,
            name: `LoraDB vs ${engine.label}`,
            item: `${SITE_URL}/benchmarks/lora-vs-${engineId}`,
          },
        ],
      },
    ],
  };

  // Engines to compare in tables — just the two of us.
  const ENGINES_PAIR = [lora, engine];

  return (
    <Layout
      title={`LoraDB vs ${engine.label}`}
      description={`Head-to-head benchmark: LoraDB against ${engine.label} across 82 workloads in 12 groups.`}
      wrapperClassName={styles.wrapper}
    >
      <Head>
        <script type="application/ld+json">
          {JSON.stringify(structuredData)}
        </script>
      </Head>

      <main className={styles.page}>
        <BenchmarkHero
          eyebrow={`vs ${engine.label}`}
          title={`LoraDB vs`}
          highlight={engine.label}
          lede={
            <>
              {headline}{" "}
              {tagline ? <>{`${engine.label} is an ${tagline}. `}</> : null}
              Numbers from the same suite as the{" "}
              <Link to="/benchmarks">overview</Link> — identical fixtures,
              identical seed, identical iteration count.
            </>
          }
          meta={
            <>
              <span>{breakdown.totalWorkloads} comparable workloads</span>
              <span>{omitted.length} omissions</span>
              <span>
                {
                  GROUPS.filter((g) =>
                    (WORKLOADS[g.id] ?? []).some(
                      (w) => w.results.lora?.time && w.results[engineId]?.time,
                    ),
                  ).length
                }{" "}
                groups
              </span>
            </>
          }
          stats={
            <>
              <SummaryStat
                accent
                value={
                  typeof totalSlowdown === "number"
                    ? totalSlowdownDisplay
                    : "1×"
                }
                unit={typeof totalSlowdown === "number" ? "slower" : ""}
                label={`${engine.label} vs LoraDB overall`}
                hint="Geometric-mean slowdown across the full suite."
              />
              <SummaryStat
                value={`${breakdown.loraWins}`}
                unit={`/ ${breakdown.totalWorkloads}`}
                label="Workloads won by LoraDB"
                hint={
                  breakdown.competitorWins > 0
                    ? `${engine.label} wins ${breakdown.competitorWins}; ${breakdown.otherWins} go to a third engine.`
                    : `${breakdown.otherWins} go to a third engine.`
                }
              />
              <SummaryStat
                value={`${breakdown.closeCalls}`}
                unit="close calls"
                label="Within ~10% on both sides"
                hint="Workloads where neither engine ran away with the row."
              />
            </>
          }
        />

        <section
          ref={navRef}
          className={clsx(
            styles.navSection,
            navHidden && styles.navSectionLifted,
          )}
          aria-label="Benchmark pages"
        >
          <div className={styles.sectionInner}>
            <div className={styles.navRow}>
              <CompetitorNav active={engineId} />
              <p className={styles.navHint}>
                Same suite as the overview — only the comparison engine differs.
              </p>
            </div>
          </div>
        </section>

        <section
          className={styles.competitorSummary}
          aria-labelledby="competitor-summary-title"
        >
          <div className={styles.sectionInner}>
            <p className={styles.sectionEyebrow}>Overall</p>
            <h2 id="competitor-summary-title" className={styles.sectionTitle}>
              LoraDB sets the row; {engine.label} sits at{" "}
              {typeof totalSlowdown === "number"
                ? `${formatRatio(totalSlowdown)} slower`
                : "—"}
              .
            </h2>
            <p className={styles.sectionLede}>
              Geometric-mean slowdown across every workload both engines run.
              Empty rows happen when the language can&rsquo;t express the
              workload — they&rsquo;re called out below, not hidden.
            </p>
            <div className={styles.barCard}>
              <SlowdownBar
                engine="lora"
                value="fastest"
                label={lora.label}
                max={100}
              />
              <SlowdownBar
                engine={engineId}
                value={totalSlowdown}
                label={engine.label}
                max={100}
              />
            </div>
          </div>
        </section>

        <section
          className={styles.headToHead}
          aria-labelledby="head-to-head-title"
        >
          <div className={styles.sectionInner}>
            <p className={styles.sectionEyebrow}>Per group</p>
            <h2 id="head-to-head-title" className={styles.sectionTitle}>
              Wins by group.
            </h2>
            <p className={styles.sectionLede}>
              For each group, the workload count, who wins it, and how the
              per-row tally breaks down between LoraDB, {engine.label}, and any
              third engine that took a row.
            </p>

            <ul className={styles.h2hList}>
              {breakdown.groups.map(
                ({
                  group,
                  comparable,
                  loraWins,
                  competitorWins,
                  otherWins,
                }) => {
                  const groupSummaryLora = group.summary.lora;
                  const groupSummaryComp = group.summary[engineId];
                  const loraScore =
                    groupSummaryLora === "fastest"
                      ? "fastest"
                      : typeof groupSummaryLora === "number"
                        ? `${formatRatio(groupSummaryLora)} slower`
                        : "n/a";
                  const compScore =
                    groupSummaryComp === "fastest"
                      ? "fastest"
                      : typeof groupSummaryComp === "number"
                        ? `${formatRatio(groupSummaryComp)} slower`
                        : "n/a";
                  return (
                    <li key={group.id} className={styles.h2hRow}>
                      <div className={styles.h2hName}>
                        <span className={styles.h2hNameLabel}>
                          {group.name}
                        </span>
                        <span className={styles.h2hNameSub}>
                          {comparable.length}{" "}
                          {comparable.length === 1 ? "workload" : "workloads"} ·
                          winner{" "}
                          {ENGINE_BY_ID[group.winner]?.label ?? group.winner}
                        </span>
                      </div>
                      <div className={styles.h2hScores}>
                        <div
                          className={clsx(styles.h2hScore, styles.h2hScoreLora)}
                        >
                          <span className={styles.h2hScoreLabel}>
                            {lora.label}
                          </span>
                          <span className={styles.h2hScoreValue}>
                            {loraScore}
                          </span>
                        </div>
                        <div className={styles.h2hScore}>
                          <span className={styles.h2hScoreLabel}>
                            {engine.label}
                          </span>
                          <span className={styles.h2hScoreValue}>
                            {compScore}
                          </span>
                        </div>
                      </div>
                      <div className={styles.h2hWin}>
                        <span className={styles.h2hWinHeader}>
                          Rows · {comparable.length}
                        </span>
                        <span className={styles.h2hWinScoreCounts}>
                          <span className={styles.h2hWinScoreCountsLora}>
                            {loraWins}
                          </span>
                          <span className={styles.h2hWinScoreDivider}>·</span>
                          <span>{competitorWins}</span>
                          {otherWins > 0 ? (
                            <>
                              <span className={styles.h2hWinScoreDivider}>
                                ·
                              </span>
                              <span>{otherWins}</span>
                            </>
                          ) : null}
                        </span>
                      </div>
                    </li>
                  );
                },
              )}
            </ul>
          </div>
        </section>

        <section className={styles.workloads} aria-labelledby="workloads-title">
          <div className={styles.sectionInner}>
            <p className={styles.sectionEyebrow}>Per workload</p>
            <h2 id="workloads-title" className={styles.sectionTitle}>
              Raw timings, workload by workload.
            </h2>
            <p className={styles.sectionLede}>
              Two columns — LoraDB and {engine.label} — across every workload
              they share. Rows with no comparable {engine.label}
              run are hidden here; they&rsquo;re listed in the omissions block
              at the bottom.
            </p>

            <div className={styles.groupSections}>
              {breakdown.groups.map(({ group, comparable }) => (
                <section
                  key={group.id}
                  id={`group-${group.id}`}
                  className={styles.groupSection}
                  aria-labelledby={`group-${group.id}-title`}
                >
                  <header className={styles.groupSectionHeader}>
                    <h3
                      id={`group-${group.id}-title`}
                      className={styles.groupSectionTitle}
                    >
                      {group.name}
                      <span className={styles.groupSectionCount}>
                        ({comparable.length})
                      </span>
                    </h3>
                    <span className={styles.groupSectionWinner}>
                      <span className={styles.groupSectionWinnerLabel}>
                        Winner
                      </span>
                      <span
                        className={
                          group.winner === "lora"
                            ? styles.groupSectionWinnerLora
                            : styles.groupSectionWinnerOther
                        }
                      >
                        {ENGINE_BY_ID[group.winner]?.label ?? group.winner}
                      </span>
                    </span>
                  </header>

                  <BenchmarkTable
                    engines={ENGINES_PAIR}
                    compact
                    rows={comparable.map((w) => ({
                      key: w.name,
                      label: w.name,
                      size: w.size,
                      // For per-competitor tables we still keep the row's
                      // overall winner — even when neither lora nor the
                      // competitor took the row — so the reader sees the
                      // honest result.
                      winner: w.winner,
                      results: {
                        lora: w.results.lora,
                        [engineId]: w.results[engineId],
                      },
                    }))}
                  />
                </section>
              ))}
            </div>
          </div>
        </section>

        {omitted.length > 0 ? (
          <section
            className={styles.disclosure}
            aria-labelledby="omitted-title"
          >
            <div className={styles.sectionInner}>
              <p className={styles.sectionEyebrow}>Honest omissions</p>
              <h2 id="omitted-title" className={styles.sectionTitle}>
                Workloads {engine.label} couldn&rsquo;t run like-for-like.
              </h2>
              <OmittedNote notes={omitted} />
            </div>
          </section>
        ) : null}
      </main>
    </Layout>
  );
}

function SummaryStat({ accent, value, unit, label, hint }) {
  return (
    <div
      className={clsx(styles.summaryCard, accent && styles.summaryCardAccent)}
    >
      <div className={styles.summaryCardHeader}>
        <span
          className={clsx(
            styles.summaryValue,
            accent && styles.summaryValueLora,
          )}
        >
          {value}
        </span>
        {unit ? <span className={styles.summaryUnit}>{unit}</span> : null}
      </div>
      <div className={styles.summaryLabel}>{label}</div>
      {hint ? <div className={styles.summaryHint}>{hint}</div> : null}
    </div>
  );
}
