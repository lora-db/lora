import React from "react";
import clsx from "clsx";
import Layout from "@theme/Layout";
import Head from "@docusaurus/Head";
import Link from "@docusaurus/Link";

import BenchmarkHero, {
  HeroStat,
} from "@site/src/components/Benchmarks/BenchmarkHero";
import BenchmarkTable from "@site/src/components/Benchmarks/BenchmarkTable";
import CompetitorNav from "@site/src/components/Benchmarks/CompetitorNav";
import GroupCard from "@site/src/components/Benchmarks/GroupCard";
import OmittedNote from "@site/src/components/Benchmarks/OmittedNote";
import SlowdownBar from "@site/src/components/Benchmarks/SlowdownBar";

import {
  COMPETITORS,
  ENGINES,
  GROUPS,
  NOTES,
  SUITE_META,
  TOTAL_ROW,
  WORKLOADS,
  competitorsByTotal,
} from "@site/src/lib/benchmarks";
import useNavbarHide from "@site/src/lib/useNavbarHide";

import styles from "./benchmarks.module.scss";

const SITE_URL = "https://loradb.com";

const STRUCTURED_DATA = {
  "@context": "https://schema.org",
  "@graph": [
    {
      "@type": "WebPage",
      "@id": `${SITE_URL}/benchmarks#webpage`,
      url: `${SITE_URL}/benchmarks`,
      name: "LoraDB benchmarks — 82 workloads across 12 groups",
      description:
        "How LoraDB compares against Kuzu, Grafeo, SurrealDB, Memgraph, Neo4j, and HelixDB across 82 workloads in 12 benchmark groups.",
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
      ],
    },
  ],
};

// "Strongest areas" — the chip strip on the hero. Order preserved so
// it matches the order rows appear in the suite.
const STRONGEST = SUITE_META.strongestGroups;

const RANKED = competitorsByTotal();

// Rows for the overall summary table — each group plus the bold
// "total" row at the bottom. Reused shape: BenchmarkTable rows.
const OVERVIEW_ROWS = [
  ...GROUPS.map((g) => ({
    key: g.id,
    label: g.name,
    sub: null,
    size: g.workloadCount,
    winner: g.winner,
    results: g.summary,
  })),
  {
    key: "total",
    label: "total",
    sub: null,
    size: TOTAL_ROW.workloadCount,
    winner: TOTAL_ROW.winner,
    results: TOTAL_ROW.summary,
  },
];

export default function Benchmarks() {
  const [navRef, navHidden] = useNavbarHide();
  return (
    <Layout
      title="Benchmarks"
      description="LoraDB benchmarked against Kuzu, Grafeo, SurrealDB, Memgraph, Neo4j, and HelixDB across 82 workloads in 12 groups."
      wrapperClassName={styles.wrapper}
    >
      <Head>
        <script type="application/ld+json">
          {JSON.stringify(STRUCTURED_DATA)}
        </script>
      </Head>

      <main className={styles.page}>
        <BenchmarkHero
          eyebrow="In this benchmark run"
          title="LoraDB is fastest overall across"
          highlight="82 workloads"
          lede="Geometric-mean slowdown per engine across every workload they share. LoraDB wins eight of twelve groups; Grafeo takes setup and writes; Kuzu takes strings and numerics. Numbers here come straight from comparisons/report.md — same suite, same fixtures, same seed."
          meta={
            <>
              <span>{SUITE_META.totalWorkloads} workloads</span>
              <span>{SUITE_META.totalGroups} groups</span>
              <span>{COMPETITORS.length} comparison engines</span>
            </>
          }
          stats={
            <>
              <HeroStat
                accent
                value="1×"
                label="LoraDB total"
                hint="Fastest overall across this suite."
              />
              <HeroStat
                value={`${SUITE_META.totalWorkloads}`}
                label="Workloads"
                hint={`${SUITE_META.totalGroups} groups · 6 engines compared`}
              />
              <HeroStat
                value={`${SUITE_META.strongestGroups.length}/${SUITE_META.totalGroups}`}
                label="Groups won by LoraDB"
                hint="Grafeo wins 2; Kuzu wins 2."
              />
              <HeroStat
                value={`${formatLeadFactor()}`}
                label="vs. next-best total"
                hint={`Next-best (${nextBest().label}) ${nextBest().slowdown.toFixed(2)}× slower overall.`}
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
              <CompetitorNav active="overview" />
              <p className={styles.navHint}>
                Dive into a head-to-head — each page compares only LoraDB
                against that engine.
              </p>
            </div>
          </div>
        </section>

        <section
          id="strongest"
          className={styles.strongest}
          aria-labelledby="strongest-title"
        >
          <div className={styles.sectionInner}>
            <p className={styles.sectionEyebrow}>Where LoraDB leads</p>
            <h2 id="strongest-title" className={styles.sectionTitle}>
              Strongest across scans, predicates, traversals, and patterns.
            </h2>
            <p className={styles.sectionLede}>
              LoraDB wins the group on every workload it touches in these areas.
              Strings and numerics belong to Kuzu (LoraDB is within ~22% on
              both); writes and setup belong to Grafeo (LoraDB pays roughly 2×
              on writes against Grafeo&rsquo;s purpose-built crate path).
            </p>
            <ul className={styles.strongestChips}>
              {STRONGEST.map((id) => {
                const g = GROUPS.find((gg) => gg.id === id);
                return (
                  <li key={id} className={styles.strongestChip}>
                    <span className={styles.strongestChipName}>{g.name}</span>
                    <span className={styles.strongestChipCount}>
                      {g.workloadCount}{" "}
                      {g.workloadCount === 1 ? "workload" : "workloads"}
                    </span>
                  </li>
                );
              })}
            </ul>
          </div>
        </section>

        <section
          id="ranking"
          className={styles.ranking}
          aria-labelledby="ranking-title"
        >
          <div className={styles.sectionInner}>
            <p className={styles.sectionEyebrow}>Overall ranking</p>
            <h2 id="ranking-title" className={styles.sectionTitle}>
              Total geometric-mean slowdown, ordered fastest to slowest.
            </h2>
            <div className={styles.barCard}>
              <SlowdownBar
                engine="lora"
                value="fastest"
                label="LoraDB"
                max={100}
              />
              {RANKED.map(({ engine, slowdown }) => (
                <SlowdownBar
                  key={engine.id}
                  engine={engine.id}
                  value={slowdown}
                  label={engine.label}
                  max={100}
                />
              ))}
            </div>
            <p className={styles.disclaimer}>
              Log scale. Bar length reflects log₁₀(slowdown) so multiples
              spanning 2.3×–57× remain visually comparable. Slowdowns are copied
              verbatim from{" "}
              <Link to="https://github.com/lora-db/lora/blob/main/comparisons/report.md">
                comparisons/report.md
              </Link>
              .
            </p>
          </div>
        </section>

        <section
          id="groups"
          className={styles.groups}
          aria-labelledby="groups-title"
        >
          <div className={styles.sectionInner}>
            <p className={styles.sectionEyebrow}>Per group</p>
            <h2 id="groups-title" className={styles.sectionTitle}>
              Twelve groups, eighty-two workloads.
            </h2>
            <p className={styles.sectionLede}>
              Each card shows the workload count, the group winner, and the
              per-engine slowdown chip. LoraDB sits in its own row so the
              comparison stays anchored even when LoraDB is not the row winner.
            </p>
            <div className={styles.groupGrid}>
              {GROUPS.map((g) => (
                <GroupCard key={g.id} group={g} engines={ENGINES} />
              ))}
            </div>
          </div>
        </section>

        <section
          id="overview-table"
          className={styles.overviewTable}
          aria-labelledby="overview-table-title"
        >
          <div className={styles.sectionInner}>
            <p className={styles.sectionEyebrow}>Summary table</p>
            <h2 id="overview-table-title" className={styles.sectionTitle}>
              The summary in one table.
            </h2>
            <p className={styles.sectionLede}>
              Each engine column carries the geometric-mean slowdown of that
              engine vs the group winner across every workload they share. Empty
              cells are noted below.
            </p>
            <BenchmarkTable
              engines={ENGINES}
              rows={OVERVIEW_ROWS}
              winnerKey="total"
              caption="Geometric-mean slowdown by group, lower is better."
            />
          </div>
        </section>

        <section
          id="workloads"
          className={styles.workloads}
          aria-labelledby="workloads-title"
        >
          <div className={styles.sectionInner}>
            <p className={styles.sectionEyebrow}>Per-workload detail</p>
            <h2 id="workloads-title" className={styles.sectionTitle}>
              Every workload, every engine, raw timings.
            </h2>
            <p className={styles.sectionLede}>
              Workload rows show the raw mean time plus the slowdown against the
              row&rsquo;s winner. Omitted cells mean the workload has no
              like-for-like equivalent on that engine; the reason is captured
              under each group.
            </p>

            <div className={styles.groupSections}>
              {GROUPS.map((g) => (
                <section
                  key={g.id}
                  id={`group-${g.id}`}
                  className={styles.groupSection}
                  aria-labelledby={`group-${g.id}-title`}
                >
                  <header className={styles.groupSectionHeader}>
                    <h3
                      id={`group-${g.id}-title`}
                      className={styles.groupSectionTitle}
                    >
                      {g.name}
                      <span className={styles.groupSectionCount}>
                        ({g.workloadCount})
                      </span>
                    </h3>
                    <span className={styles.groupSectionWinner}>
                      <span className={styles.groupSectionWinnerLabel}>
                        Winner
                      </span>
                      <span
                        className={
                          g.winner === "lora"
                            ? styles.groupSectionWinnerLora
                            : styles.groupSectionWinnerOther
                        }
                      >
                        {ENGINES.find((e) => e.id === g.winner)?.label ??
                          g.winner}
                      </span>
                    </span>
                  </header>

                  <BenchmarkTable
                    engines={ENGINES}
                    rows={(WORKLOADS[g.id] ?? []).map((w) => ({
                      key: w.name,
                      label: w.name,
                      sub: null,
                      size: w.size,
                      winner: w.winner,
                      results: w.results,
                    }))}
                  />

                  <OmittedNote notes={NOTES.filter((n) => n.group === g.id)} />
                </section>
              ))}
            </div>
          </div>
        </section>

        <section
          id="disclosure"
          className={styles.disclosure}
          aria-labelledby="disclosure-title"
        >
          <div className={styles.sectionInner}>
            <p className={styles.sectionEyebrow}>Methodology</p>
            <h2 id="disclosure-title" className={styles.sectionTitle}>
              What this benchmark does and doesn&rsquo;t say.
            </h2>
            <div className={styles.disclosureGrid}>
              <article className={styles.disclosureCard}>
                <h3>What&rsquo;s measured</h3>
                <p>
                  Mean iteration time on identical fixtures. Engines run the
                  same query semantics where the language allows expressing
                  them; where it doesn&rsquo;t, the workload is marked omitted
                  with the reason recorded next to it. Geometric mean is used at
                  the group level so a single outlier doesn&rsquo;t dominate the
                  row.
                </p>
              </article>
              <article className={styles.disclosureCard}>
                <h3>What this isn&rsquo;t</h3>
                <p>
                  Not a production-scale TPC benchmark. Not a load test. Not a
                  comparison of operational footprint or durability — every
                  engine is run in-process or against a local server with the
                  same warm fixture each iteration. Workloads are
                  micro-benchmarks of the query pipeline, not of full systems.
                </p>
              </article>
              <article className={styles.disclosureCard}>
                <h3>Where to dig in</h3>
                <p>
                  The full source — fixtures, harness, per-engine glue — lives
                  in{" "}
                  <Link to="https://github.com/lora-db/lora/tree/main/comparisons">
                    comparisons/
                  </Link>
                  . The report itself is generated from a single Criterion run;
                  re-running the suite regenerates report.md, which this page is
                  derived from.
                </p>
              </article>
            </div>
          </div>
        </section>
      </main>
    </Layout>
  );
}

function formatLeadFactor() {
  const next = nextBest();
  if (!next) return "—";
  if (next.slowdown >= 10) return `${next.slowdown.toFixed(1)}×`;
  return `${next.slowdown.toFixed(2)}×`;
}

function nextBest() {
  const candidates = COMPETITORS.map((c) => ({
    label: c.label,
    slowdown: TOTAL_ROW.summary[c.id],
  })).filter((c) => typeof c.slowdown === "number");
  candidates.sort((a, b) => a.slowdown - b.slowdown);
  return candidates[0] ?? { label: "—", slowdown: 0 };
}
