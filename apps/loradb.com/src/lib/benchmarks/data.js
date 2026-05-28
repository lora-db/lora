// Structured benchmark data, derived from comparisons/report.md.
//
// The marketing site does not re-run the benches; whoever produces a
// new report.md updates this file by hand (or via a parser script
// added later). Keeping the data in one module — separate from the
// pages and components that render it — means a refresh is a single
// data edit, not a UI rewrite.
//
// Conventions:
//   - `slowdown` is null when an engine is the winner of the row
//     (rendered as the "fastest" badge instead of "Nx slower").
//   - `time` is null when an engine has no like-for-like equivalent
//     for the workload — rendered as a muted "omitted" cell, with the
//     reason captured in `notes` keyed by `${workload}.${engine}`.
//   - Engine order is fixed sitewide and starts with `lora` so cards
//     and tables read left-to-right with LoraDB as the anchor column.

export const ENGINES = [
  { id: "lora", label: "LoraDB", short: "Lora" },
  { id: "kuzu", label: "Kuzu", short: "Kuzu" },
  { id: "grafeo", label: "Grafeo", short: "Grafeo" },
  { id: "surrealdb", label: "SurrealDB", short: "SurrealDB" },
  { id: "memgraph", label: "Memgraph", short: "Memgraph" },
  { id: "neo4j", label: "Neo4j", short: "Neo4j" },
  { id: "helixdb", label: "HelixDB", short: "HelixDB" },
];

export const COMPETITORS = ENGINES.filter((e) => e.id !== "lora");

export const ENGINE_BY_ID = Object.fromEntries(ENGINES.map((e) => [e.id, e]));

// One-line tagline shown on each competitor page. Kept short — the
// claim "what this engine is" rather than a comparative judgement.
export const COMPETITOR_TAGLINE = {
  kuzu: "embedded columnar graph engine",
  grafeo: "in-process Rust property-graph crate",
  surrealdb: "multi-model database server",
  memgraph: "in-memory graph database server",
  neo4j: "the original Cypher graph database",
  helixdb: "graph-vector database (enterprise-dev image)",
};

// Geometric-mean slowdown per group, copied from the summary table.
// `fastest` for the winner; `null` if the engine had no comparable
// workloads in that group.
export const GROUPS = [
  {
    id: "setup",
    name: "setup",
    workloadCount: 1,
    winner: "grafeo",
    summary: {
      lora: 2.7,
      kuzu: 700.36,
      grafeo: "fastest",
      surrealdb: null,
      memgraph: 904.27,
      neo4j: 1768.78,
      helixdb: 1246.77,
    },
  },
  {
    id: "writes",
    name: "writes",
    workloadCount: 9,
    winner: "grafeo",
    summary: {
      lora: 0.61,
      kuzu: 1.79,
      grafeo: "fastest",
      surrealdb: 28.33,
      memgraph: 3.71,
      neo4j: 7.75,
      helixdb: 124.5,
    },
  },
  {
    id: "scans",
    name: "scans",
    workloadCount: 6,
    winner: "lora",
    summary: {
      lora: "fastest",
      kuzu: 6.37,
      grafeo: 5.43,
      surrealdb: 120.66,
      memgraph: 27.54,
      neo4j: 24.12,
      helixdb: 287.24,
    },
  },
  {
    id: "predicates",
    name: "predicates",
    workloadCount: 12,
    winner: "lora",
    summary: {
      lora: "fastest",
      kuzu: 1.14,
      grafeo: 1.83,
      surrealdb: 21.32,
      memgraph: 4.77,
      neo4j: 3.78,
      helixdb: 20.86,
    },
  },
  {
    id: "strings",
    name: "strings",
    workloadCount: 5,
    winner: "kuzu",
    summary: {
      lora: 1.22,
      kuzu: "fastest",
      grafeo: 1.84,
      surrealdb: 34.09,
      memgraph: 11.3,
      neo4j: 4.19,
      helixdb: null,
    },
  },
  {
    id: "numerics",
    name: "numerics",
    workloadCount: 6,
    winner: "kuzu",
    summary: {
      lora: 1.08,
      kuzu: "fastest",
      grafeo: 1.87,
      surrealdb: 39.86,
      memgraph: 10.71,
      neo4j: 4.0,
      helixdb: 20.4,
    },
  },
  {
    id: "aggregates",
    name: "aggregates",
    workloadCount: 9,
    winner: "lora",
    summary: {
      lora: "fastest",
      kuzu: 3.18,
      grafeo: 1.89,
      surrealdb: 44.74,
      memgraph: 6.25,
      neo4j: 6.94,
      helixdb: 35.49,
    },
  },
  {
    id: "pipeline",
    name: "pipeline",
    workloadCount: 9,
    winner: "lora",
    summary: {
      lora: "fastest",
      kuzu: 1.27,
      grafeo: 1.3,
      surrealdb: 36.83,
      memgraph: 4.97,
      neo4j: 3.07,
      helixdb: 17.23,
    },
  },
  {
    id: "lists",
    name: "lists",
    workloadCount: 3,
    winner: "lora",
    summary: {
      lora: "fastest",
      kuzu: 11.33,
      grafeo: 3.21,
      surrealdb: 32.28,
      memgraph: 41.56,
      neo4j: 42.23,
      helixdb: 15.01,
    },
  },
  {
    id: "sort",
    name: "sort",
    workloadCount: 3,
    winner: "lora",
    summary: {
      lora: "fastest",
      kuzu: 1.41,
      grafeo: 1.6,
      surrealdb: 31.76,
      memgraph: 4.28,
      neo4j: 3.48,
      helixdb: 16.44,
    },
  },
  {
    id: "traversals",
    name: "traversals",
    workloadCount: 15,
    winner: "lora",
    summary: {
      lora: "fastest",
      kuzu: 20.35,
      grafeo: 5.88,
      surrealdb: 110.35,
      memgraph: 22.42,
      neo4j: 17.77,
      helixdb: 132.79,
    },
  },
  {
    id: "patterns",
    name: "patterns",
    workloadCount: 4,
    winner: "lora",
    summary: {
      lora: "fastest",
      kuzu: 2.8,
      grafeo: 2.45,
      surrealdb: 190.07,
      memgraph: 7.59,
      neo4j: 4.96,
      helixdb: 22.24,
    },
  },
];

export const TOTAL_ROW = {
  workloadCount: 82,
  winner: "lora",
  summary: {
    lora: "fastest",
    kuzu: 3.3,
    grafeo: 2.3,
    surrealdb: 48.58,
    memgraph: 9.75,
    neo4j: 7.99,
    helixdb: 57.2,
  },
};

// Helper: build a workload row.
//   t(engine, time, slowdown)
//   t(engine, time)           — winner
//   t(engine, null, null)     — omitted (no comparable run)
// `time` is the human-readable string straight from report.md so the
// numbers shown on the page match the markdown report verbatim.
const W = (entries, winner) => {
  const results = {};
  for (const [engine, time, slowdown = null] of entries) {
    results[engine] = { time, slowdown };
  }
  return { winner, results };
};

// Per-workload raw timings + relative slowdowns. The order of entries
// inside W() mirrors the order in report.md.
export const WORKLOADS = {
  setup: [
    {
      name: "construct_empty",
      size: null,
      ...W(
        [
          ["lora", "4.06 µs", 2.7],
          ["kuzu", "1.05 ms", 700.36],
          ["grafeo", "1.50 µs"],
          ["memgraph", "1.36 ms", 904.27],
          ["neo4j", "2.66 ms", 1768.78],
          ["helixdb", "1.87 ms", 1246.77],
        ],
        "grafeo",
      ),
    },
  ],
  writes: [
    {
      name: "bulk_edges",
      size: 200,
      ...W(
        [
          ["lora", "606.63 µs"],
          ["kuzu", "2.01 ms", 3.31],
          ["grafeo", "19.86 ms", 32.74],
          ["memgraph", "3.18 ms", 5.24],
          ["neo4j", "2.65 ms", 4.37],
          ["helixdb", "309.68 ms", 510.49],
        ],
        "lora",
      ),
    },
    {
      name: "bulk_set_match",
      size: 1000,
      ...W(
        [
          ["lora", "552.39 µs", 1.84],
          ["kuzu", "300.03 µs"],
          ["grafeo", "315.88 µs", 1.05],
          ["surrealdb", "6.15 ms", 20.48],
          ["memgraph", "726.92 µs", 2.42],
          ["neo4j", "916.11 µs", 3.05],
          ["helixdb", "3.57 ms", 11.9],
        ],
        "kuzu",
      ),
    },
    {
      name: "delete_node",
      size: 1000,
      ...W(
        [
          ["lora", "355.70 µs", 1.47],
          ["kuzu", "598.35 µs", 2.48],
          ["grafeo", "241.43 µs"],
          ["surrealdb", "5.56 ms", 23.04],
          ["memgraph", "483.58 µs", 2.0],
          ["neo4j", "1.65 ms", 6.84],
          ["helixdb", "151.74 ms", 628.54],
        ],
        "grafeo",
      ),
    },
    {
      name: "merge_create",
      size: 1000,
      ...W(
        [
          ["lora", "112.57 µs", 1.25],
          ["kuzu", "670.68 µs", 7.45],
          ["grafeo", "90.02 µs"],
          ["memgraph", "677.96 µs", 7.53],
          ["neo4j", "2.48 ms", 27.56],
        ],
        "grafeo",
      ),
    },
    {
      name: "merge_existing",
      size: 1000,
      ...W(
        [
          ["lora", "23.95 µs", 2.89],
          ["kuzu", "162.06 µs", 19.55],
          ["grafeo", "8.29 µs"],
          ["memgraph", "304.94 µs", 36.78],
          ["neo4j", "647.72 µs", 78.13],
        ],
        "grafeo",
      ),
    },
    {
      name: "set_multiple_props",
      size: 1000,
      ...W(
        [
          ["lora", "21.15 µs"],
          ["kuzu", "169.72 µs", 8.03],
          ["grafeo", "182.77 µs", 8.64],
          ["surrealdb", "5.54 ms", 262.09],
          ["memgraph", "313.89 µs", 14.84],
          ["neo4j", "571.61 µs", 27.03],
          ["helixdb", "2.37 ms", 112.05],
        ],
        "lora",
      ),
    },
    {
      name: "update_set",
      size: 1000,
      ...W(
        [
          ["lora", "18.01 µs"],
          ["kuzu", "188.62 µs", 10.47],
          ["grafeo", "208.61 µs", 11.58],
          ["surrealdb", "6.04 ms", 335.3],
          ["memgraph", "315.49 µs", 17.52],
          ["neo4j", "732.47 µs", 40.68],
          ["helixdb", "2.91 ms", 161.53],
        ],
        "lora",
      ),
    },
    {
      name: "write_bulk",
      size: 1000,
      ...W(
        [
          ["lora", "1.46 ms", 1.93],
          ["kuzu", "2.76 ms", 3.65],
          ["grafeo", "756.21 µs"],
          ["surrealdb", "26.66 ms", 35.26],
          ["memgraph", "3.13 ms", 4.14],
          ["neo4j", "6.34 ms", 8.38],
          ["helixdb", "771.22 ms", 1019.86],
        ],
        "grafeo",
      ),
    },
    {
      name: "write_single",
      size: 1000,
      ...W(
        [
          ["lora", "14.90 µs", 2.2],
          ["grafeo", "6.78 µs"],
          ["surrealdb", "252.56 µs", 37.27],
          ["memgraph", "413.52 µs", 61.02],
          ["neo4j", "1.30 ms", 192.29],
          ["helixdb", "153.86 ms", 22704.2],
        ],
        "grafeo",
      ),
    },
  ],
  scans: [
    {
      name: "distinct",
      size: 1000,
      ...W(
        [
          ["lora", "198.38 µs"],
          ["kuzu", "448.06 µs", 2.26],
          ["grafeo", "246.37 µs", 1.24],
          ["memgraph", "738.04 µs", 3.72],
          ["neo4j", "566.24 µs", 2.85],
        ],
        "lora",
      ),
    },
    {
      name: "lookup_by_id",
      size: 1000,
      ...W(
        [
          ["lora", "716.64 ns"],
          ["kuzu", "116.07 µs", 161.96],
          ["grafeo", "170.03 µs", 237.26],
          ["surrealdb", "5.26 ms", 7340.09],
          ["memgraph", "305.39 µs", 426.14],
          ["neo4j", "601.36 µs", 839.14],
          ["helixdb", "2.30 ms", 3204.88],
        ],
        "lora",
      ),
    },
    {
      name: "lookup_by_id_indexed",
      size: 1000,
      ...W(
        [
          ["lora", "684.00 ns"],
          ["kuzu", "116.18 µs", 169.85],
          ["grafeo", "22.73 µs", 33.23],
          ["surrealdb", "32.35 µs", 47.3],
          ["memgraph", "310.02 µs", 453.25],
          ["neo4j", "747.01 µs", 1092.12],
          ["helixdb", "2.62 ms", 3830.95],
        ],
        "lora",
      ),
    },
    {
      name: "range_filter",
      size: 1000,
      ...W(
        [
          ["lora", "199.70 µs", 1.04],
          ["kuzu", "192.68 µs"],
          ["grafeo", "214.22 µs", 1.11],
          ["surrealdb", "9.29 ms", 48.24],
          ["memgraph", "1.22 ms", 6.31],
          ["neo4j", "589.16 µs", 3.06],
        ],
        "kuzu",
      ),
    },
    {
      name: "scan_filtered",
      size: 1000,
      ...W(
        [
          ["lora", "149.08 µs"],
          ["kuzu", "158.49 µs", 1.06],
          ["grafeo", "211.95 µs", 1.42],
          ["surrealdb", "5.88 ms", 39.46],
          ["memgraph", "1.15 ms", 7.73],
          ["neo4j", "718.45 µs", 4.82],
          ["helixdb", "3.82 ms", 25.64],
        ],
        "lora",
      ),
    },
    {
      name: "scan_label",
      size: 1000,
      ...W(
        [
          ["lora", "124.96 µs"],
          ["kuzu", "131.34 µs", 1.05],
          ["grafeo", "213.71 µs", 1.71],
          ["surrealdb", "5.01 ms", 40.1],
          ["memgraph", "1.61 ms", 12.89],
          ["neo4j", "661.84 µs", 5.3],
          ["helixdb", "2.70 ms", 21.62],
        ],
        "lora",
      ),
    },
  ],
  predicates: [
    {
      name: "where_compound_and_or",
      size: 1000,
      ...W(
        [
          ["lora", "213.07 µs"],
          ["kuzu", "230.83 µs", 1.08],
          ["grafeo", "646.50 µs", 3.03],
          ["surrealdb", "10.23 ms", 48.02],
          ["memgraph", "997.22 µs", 4.68],
          ["neo4j", "584.96 µs", 2.75],
          ["helixdb", "3.23 ms", 15.17],
        ],
        "lora",
      ),
    },
    {
      name: "where_contains",
      size: 1000,
      ...W(
        [
          ["lora", "145.04 µs"],
          ["kuzu", "166.24 µs", 1.15],
          ["grafeo", "255.53 µs", 1.76],
          ["surrealdb", "4.96 ms", 34.2],
          ["memgraph", "638.95 µs", 4.41],
          ["neo4j", "597.31 µs", 4.12],
          ["helixdb", "3.02 ms", 20.84],
        ],
        "lora",
      ),
    },
    {
      name: "where_ends_with",
      size: 1000,
      ...W(
        [
          ["lora", "143.60 µs"],
          ["kuzu", "167.41 µs", 1.17],
          ["grafeo", "236.22 µs", 1.64],
          ["surrealdb", "5.06 ms", 35.23],
          ["memgraph", "726.66 µs", 5.06],
          ["neo4j", "581.26 µs", 4.05],
          ["helixdb", "2.99 ms", 20.8],
        ],
        "lora",
      ),
    },
    {
      name: "where_id_in_range",
      size: 1000,
      ...W(
        [
          ["lora", "142.71 µs", 2.02],
          ["kuzu", "187.97 µs", 2.66],
          ["grafeo", "70.55 µs"],
          ["surrealdb", "7.54 ms", 106.85],
          ["memgraph", "433.95 µs", 6.15],
          ["neo4j", "663.12 µs", 9.4],
          ["helixdb", "3.13 ms", 44.34],
        ],
        "grafeo",
      ),
    },
    {
      name: "where_in_list",
      size: 1000,
      ...W(
        [
          ["lora", "162.42 µs"],
          ["kuzu", "196.06 µs", 1.21],
          ["grafeo", "270.30 µs", 1.66],
          ["surrealdb", "5.59 ms", 34.4],
          ["memgraph", "606.63 µs", 3.74],
          ["neo4j", "627.53 µs", 3.86],
          ["helixdb", "2.92 ms", 17.99],
        ],
        "lora",
      ),
    },
    {
      name: "where_modulo_eq",
      size: 1000,
      ...W(
        [
          ["lora", "127.32 µs"],
          ["kuzu", "167.83 µs", 1.32],
          ["grafeo", "250.88 µs", 1.97],
          ["surrealdb", "128.48 µs", 1.01],
          ["memgraph", "717.29 µs", 5.63],
          ["neo4j", "557.09 µs", 4.38],
          ["helixdb", "3.01 ms", 23.66],
        ],
        "lora",
      ),
    },
    {
      name: "where_not",
      size: 1000,
      ...W(
        [
          ["lora", "166.05 µs", 1.01],
          ["kuzu", "164.07 µs"],
          ["grafeo", "340.52 µs", 2.08],
          ["surrealdb", "8.27 ms", 50.4],
          ["memgraph", "1.24 ms", 7.57],
          ["neo4j", "695.07 µs", 4.24],
          ["helixdb", "4.64 ms", 28.27],
        ],
        "kuzu",
      ),
    },
    {
      name: "where_or",
      size: 1000,
      ...W(
        [
          ["lora", "147.16 µs"],
          ["kuzu", "185.19 µs", 1.26],
          ["grafeo", "450.08 µs", 3.06],
          ["surrealdb", "8.43 ms", 57.26],
          ["memgraph", "627.82 µs", 4.27],
          ["neo4j", "603.10 µs", 4.1],
          ["helixdb", "3.29 ms", 22.34],
        ],
        "lora",
      ),
    },
    {
      name: "where_starts_with",
      size: 1000,
      ...W(
        [
          ["lora", "146.91 µs"],
          ["kuzu", "168.08 µs", 1.14],
          ["grafeo", "249.22 µs", 1.7],
          ["surrealdb", "5.12 ms", 34.86],
          ["memgraph", "727.06 µs", 4.95],
          ["neo4j", "596.02 µs", 4.06],
          ["helixdb", "3.29 ms", 22.38],
        ],
        "lora",
      ),
    },
    {
      name: "where_string_gte",
      size: 1000,
      ...W(
        [
          ["lora", "180.97 µs", 1.07],
          ["kuzu", "168.40 µs"],
          ["grafeo", "247.34 µs", 1.47],
          ["surrealdb", "6.05 ms", 35.91],
          ["memgraph", "1.20 ms", 7.1],
          ["neo4j", "612.78 µs", 3.64],
          ["helixdb", "4.11 ms", 24.43],
        ],
        "kuzu",
      ),
    },
    {
      name: "where_subexpr",
      size: 1000,
      ...W(
        [
          ["lora", "227.59 µs", 1.69],
          ["kuzu", "194.85 µs", 1.45],
          ["grafeo", "589.01 µs", 4.37],
          ["surrealdb", "134.71 µs"],
          ["memgraph", "1.76 ms", 13.08],
          ["neo4j", "584.16 µs", 4.34],
          ["helixdb", "4.73 ms", 35.11],
        ],
        "surrealdb",
      ),
    },
    {
      name: "where_two_props",
      size: 1000,
      ...W(
        [
          ["lora", "152.69 µs"],
          ["kuzu", "205.41 µs", 1.35],
          ["grafeo", "404.15 µs", 2.65],
          ["surrealdb", "6.46 ms", 42.31],
          ["memgraph", "390.30 µs", 2.56],
          ["neo4j", "589.64 µs", 3.86],
          ["helixdb", "2.56 ms", 16.77],
        ],
        "lora",
      ),
    },
  ],
  strings: [
    {
      name: "string_concat",
      size: 1000,
      ...W(
        [
          ["lora", "186.55 µs", 1.27],
          ["kuzu", "147.24 µs"],
          ["grafeo", "283.23 µs", 1.92],
          ["surrealdb", "5.86 ms", 39.77],
          ["memgraph", "1.68 ms", 11.38],
          ["neo4j", "577.43 µs", 3.92],
        ],
        "kuzu",
      ),
    },
    {
      name: "string_size",
      size: 1000,
      ...W(
        [
          ["lora", "172.17 µs", 1.1],
          ["kuzu", "156.38 µs"],
          ["grafeo", "251.12 µs", 1.61],
          ["surrealdb", "5.23 ms", 33.47],
          ["memgraph", "1.70 ms", 10.9],
          ["neo4j", "567.80 µs", 3.63],
        ],
        "kuzu",
      ),
    },
    {
      name: "string_substring",
      size: 1000,
      ...W(
        [
          ["lora", "210.20 µs", 1.21],
          ["kuzu", "173.57 µs"],
          ["grafeo", "314.23 µs", 1.81],
          ["surrealdb", "5.58 ms", 32.15],
          ["memgraph", "2.06 ms", 11.88],
          ["neo4j", "877.36 µs", 5.05],
        ],
        "kuzu",
      ),
    },
    {
      name: "string_to_lower",
      size: 1000,
      ...W(
        [
          ["lora", "201.00 µs", 1.33],
          ["kuzu", "151.22 µs"],
          ["grafeo", "297.66 µs", 1.97],
          ["surrealdb", "5.03 ms", 33.29],
          ["memgraph", "1.72 ms", 11.36],
          ["neo4j", "684.21 µs", 4.52],
        ],
        "kuzu",
      ),
    },
    {
      name: "string_to_upper",
      size: 1000,
      ...W(
        [
          ["lora", "185.41 µs", 1.22],
          ["kuzu", "151.96 µs"],
          ["grafeo", "292.95 µs", 1.93],
          ["surrealdb", "4.91 ms", 32.34],
          ["memgraph", "1.67 ms", 11.02],
          ["neo4j", "603.96 µs", 3.97],
        ],
        "kuzu",
      ),
    },
  ],
  numerics: [
    {
      name: "numeric_abs",
      size: 1000,
      ...W(
        [
          ["lora", "174.85 µs", 1.13],
          ["kuzu", "154.99 µs"],
          ["grafeo", "272.48 µs", 1.76],
          ["surrealdb", "5.86 ms", 37.81],
          ["memgraph", "1.64 ms", 10.59],
          ["neo4j", "636.05 µs", 4.1],
        ],
        "kuzu",
      ),
    },
    {
      name: "numeric_ceil",
      size: 1000,
      ...W(
        [
          ["lora", "171.69 µs", 1.1],
          ["kuzu", "155.40 µs"],
          ["grafeo", "269.13 µs", 1.73],
          ["surrealdb", "5.81 ms", 37.36],
          ["memgraph", "1.78 ms", 11.46],
          ["neo4j", "634.65 µs", 4.08],
        ],
        "kuzu",
      ),
    },
    {
      name: "numeric_floor",
      size: 1000,
      ...W(
        [
          ["lora", "175.48 µs", 1.1],
          ["kuzu", "159.62 µs"],
          ["grafeo", "326.82 µs", 2.05],
          ["surrealdb", "5.99 ms", 37.52],
          ["memgraph", "1.64 ms", 10.29],
          ["neo4j", "609.30 µs", 3.82],
        ],
        "kuzu",
      ),
    },
    {
      name: "numeric_modulo",
      size: 1000,
      ...W(
        [
          ["lora", "144.95 µs", 1.02],
          ["kuzu", "142.67 µs"],
          ["grafeo", "236.39 µs", 1.66],
          ["memgraph", "1.59 ms", 11.17],
          ["neo4j", "637.61 µs", 4.47],
          ["helixdb", "2.97 ms", 20.79],
        ],
        "kuzu",
      ),
    },
    {
      name: "numeric_pow",
      size: 1000,
      ...W(
        [
          ["lora", "166.22 µs", 1.16],
          ["kuzu", "143.34 µs"],
          ["grafeo", "394.61 µs", 2.75],
          ["surrealdb", "8.42 ms", 58.75],
          ["memgraph", "1.68 ms", 11.73],
          ["neo4j", "641.94 µs", 4.48],
          ["helixdb", "2.87 ms", 20.0],
        ],
        "kuzu",
      ),
    },
    {
      name: "numeric_round",
      size: 1000,
      ...W(
        [
          ["lora", "180.68 µs"],
          ["kuzu", "181.69 µs", 1.01],
          ["grafeo", "269.75 µs", 1.49],
          ["surrealdb", "5.87 ms", 32.48],
          ["memgraph", "1.68 ms", 9.3],
          ["neo4j", "581.78 µs", 3.22],
        ],
        "lora",
      ),
    },
  ],
  aggregates: [
    {
      name: "aggregate_avg",
      size: 1000,
      ...W(
        [
          ["lora", "81.43 µs"],
          ["kuzu", "253.54 µs", 3.11],
          ["grafeo", "202.26 µs", 2.48],
          ["surrealdb", "6.22 ms", 76.38],
          ["memgraph", "562.93 µs", 6.91],
          ["neo4j", "606.76 µs", 7.45],
          ["helixdb", "3.20 ms", 39.29],
        ],
        "lora",
      ),
    },
    {
      name: "aggregate_collect",
      size: 1000,
      ...W(
        [
          ["lora", "78.90 µs"],
          ["kuzu", "275.13 µs", 3.49],
          ["grafeo", "214.58 µs", 2.72],
          ["surrealdb", "6.79 ms", 86.03],
          ["memgraph", "599.26 µs", 7.6],
          ["neo4j", "619.24 µs", 7.85],
        ],
        "lora",
      ),
    },
    {
      name: "aggregate_count",
      size: 1000,
      ...W(
        [
          ["lora", "59.50 µs", 2.77],
          ["kuzu", "250.00 µs", 11.66],
          ["grafeo", "21.44 µs"],
          ["surrealdb", "209.05 µs", 9.75],
          ["memgraph", "381.02 µs", 17.77],
          ["neo4j", "588.34 µs", 27.44],
        ],
        "grafeo",
      ),
    },
    {
      name: "aggregate_count_distinct",
      size: 1000,
      ...W(
        [
          ["lora", "104.05 µs"],
          ["kuzu", "441.70 µs", 4.24],
          ["grafeo", "213.10 µs", 2.05],
          ["memgraph", "540.27 µs", 5.19],
          ["neo4j", "635.68 µs", 6.11],
        ],
        "lora",
      ),
    },
    {
      name: "aggregate_max",
      size: 1000,
      ...W(
        [
          ["lora", "79.97 µs"],
          ["kuzu", "263.33 µs", 3.29],
          ["grafeo", "200.95 µs", 2.51],
          ["surrealdb", "6.08 ms", 75.98],
          ["memgraph", "572.73 µs", 7.16],
          ["neo4j", "619.56 µs", 7.75],
          ["helixdb", "2.86 ms", 35.74],
        ],
        "lora",
      ),
    },
    {
      name: "aggregate_min",
      size: 1000,
      ...W(
        [
          ["lora", "79.67 µs"],
          ["kuzu", "258.01 µs", 3.24],
          ["grafeo", "197.17 µs", 2.47],
          ["surrealdb", "5.95 ms", 74.66],
          ["memgraph", "552.13 µs", 6.93],
          ["neo4j", "563.60 µs", 7.07],
          ["helixdb", "2.49 ms", 31.3],
        ],
        "lora",
      ),
    },
    {
      name: "aggregate_sum",
      size: 1000,
      ...W(
        [
          ["lora", "78.09 µs"],
          ["kuzu", "263.06 µs", 3.37],
          ["grafeo", "199.65 µs", 2.56],
          ["surrealdb", "6.23 ms", 79.76],
          ["memgraph", "524.56 µs", 6.72],
          ["neo4j", "568.81 µs", 7.28],
          ["helixdb", "2.82 ms", 36.08],
        ],
        "lora",
      ),
    },
    {
      name: "grouped_aggregation",
      size: 1000,
      ...W(
        [
          ["lora", "154.46 µs"],
          ["kuzu", "549.19 µs", 3.56],
          ["grafeo", "269.45 µs", 1.74],
          ["memgraph", "751.51 µs", 4.87],
          ["neo4j", "969.01 µs", 6.27],
        ],
        "lora",
      ),
    },
    {
      name: "top_k",
      size: 1000,
      ...W(
        [
          ["lora", "186.50 µs"],
          ["kuzu", "248.58 µs", 1.33],
          ["grafeo", "424.61 µs", 2.28],
          ["surrealdb", "6.40 ms", 34.33],
          ["memgraph", "955.44 µs", 5.12],
          ["neo4j", "792.15 µs", 4.25],
        ],
        "lora",
      ),
    },
  ],
  pipeline: [
    {
      name: "case_when",
      size: 1000,
      ...W(
        [
          ["lora", "173.33 µs"],
          ["kuzu", "186.25 µs", 1.07],
          ["grafeo", "256.32 µs", 1.48],
          ["surrealdb", "7.38 ms", 42.6],
          ["memgraph", "1.86 ms", 10.75],
          ["neo4j", "796.60 µs", 4.6],
          ["helixdb", "2.78 ms", 16.05],
        ],
        "lora",
      ),
    },
    {
      name: "coalesce_existing",
      size: 1000,
      ...W(
        [
          ["lora", "161.92 µs"],
          ["kuzu", "162.41 µs", 1.0],
          ["grafeo", "260.13 µs", 1.61],
          ["surrealdb", "7.51 ms", 46.39],
          ["memgraph", "1.62 ms", 9.99],
          ["neo4j", "628.36 µs", 3.88],
          ["helixdb", "3.32 ms", 20.5],
        ],
        "lora",
      ),
    },
    {
      name: "computed_in_return",
      size: 1000,
      ...W(
        [
          ["lora", "151.15 µs", 1.07],
          ["kuzu", "140.92 µs"],
          ["grafeo", "241.22 µs", 1.71],
          ["surrealdb", "7.35 ms", 52.15],
          ["memgraph", "1.61 ms", 11.46],
          ["neo4j", "639.09 µs", 4.54],
          ["helixdb", "2.99 ms", 21.2],
        ],
        "kuzu",
      ),
    },
    {
      name: "distinct_with_order",
      size: 1000,
      ...W(
        [
          ["lora", "508.01 µs", 2.08],
          ["kuzu", "549.89 µs", 2.25],
          ["grafeo", "243.88 µs"],
          ["memgraph", "692.42 µs", 2.84],
          ["neo4j", "617.76 µs", 2.53],
        ],
        "grafeo",
      ),
    },
    {
      name: "predicate_via_function",
      size: 1000,
      ...W(
        [
          ["lora", "238.39 µs", 1.38],
          ["kuzu", "172.38 µs"],
          ["grafeo", "434.70 µs", 2.52],
          ["surrealdb", "7.45 ms", 43.19],
          ["memgraph", "1.77 ms", 10.29],
          ["neo4j", "544.38 µs", 3.16],
        ],
        "kuzu",
      ),
    },
    {
      name: "with_aggregate_then_filter",
      size: 1000,
      ...W(
        [
          ["lora", "148.70 µs"],
          ["kuzu", "475.77 µs", 3.2],
          ["grafeo", "267.49 µs", 1.8],
          ["memgraph", "567.44 µs", 3.82],
          ["neo4j", "710.00 µs", 4.77],
        ],
        "lora",
      ),
    },
    {
      name: "with_distinct_then_count",
      size: 1000,
      ...W(
        [
          ["lora", "202.72 µs"],
          ["kuzu", "540.85 µs", 2.67],
          ["grafeo", "244.75 µs", 1.21],
          ["memgraph", "597.19 µs", 2.95],
          ["neo4j", "812.09 µs", 4.01],
        ],
        "lora",
      ),
    },
    {
      name: "with_pipeline",
      size: 1000,
      ...W(
        [
          ["lora", "186.41 µs"],
          ["kuzu", "333.60 µs", 1.79],
          ["grafeo", "218.95 µs", 1.17],
          ["surrealdb", "5.98 ms", 32.1],
          ["memgraph", "678.15 µs", 3.64],
          ["neo4j", "581.26 µs", 3.12],
        ],
        "lora",
      ),
    },
    {
      name: "with_two_chained",
      size: 1000,
      ...W(
        [
          ["lora", "313.64 µs", 1.41],
          ["kuzu", "222.76 µs"],
          ["grafeo", "388.95 µs", 1.75],
          ["surrealdb", "8.12 ms", 36.46],
          ["memgraph", "1.22 ms", 5.46],
          ["neo4j", "608.48 µs", 2.73],
          ["helixdb", "4.25 ms", 19.06],
        ],
        "kuzu",
      ),
    },
  ],
  lists: [
    {
      name: "list_in_construction",
      size: 1000,
      ...W(
        [
          ["lora", "177.94 µs", 1.05],
          ["kuzu", "168.89 µs"],
          ["grafeo", "450.42 µs", 2.67],
          ["surrealdb", "6.14 ms", 36.36],
          ["memgraph", "2.05 ms", 12.12],
          ["neo4j", "733.38 µs", 4.34],
          ["helixdb", "2.67 ms", 15.81],
        ],
        "kuzu",
      ),
    },
    {
      name: "list_unwind_explicit",
      size: 1000,
      ...W(
        [
          ["lora", "1.11 µs"],
          ["kuzu", "165.87 µs", 149.4],
          ["grafeo", "7.11 µs", 6.4],
          ["surrealdb", "33.52 µs", 30.19],
          ["memgraph", "322.60 µs", 290.57],
          ["neo4j", "689.53 µs", 621.07],
        ],
        "lora",
      ),
    },
    {
      name: "range_function",
      size: 1000,
      ...W(
        [
          ["lora", "18.86 µs"],
          ["kuzu", "193.49 µs", 10.26],
          ["grafeo", "38.62 µs", 2.05],
          ["memgraph", "404.85 µs", 21.46],
          ["neo4j", "555.15 µs", 29.43],
        ],
        "lora",
      ),
    },
  ],
  sort: [
    {
      name: "order_by_id_asc",
      size: 1000,
      ...W(
        [
          ["lora", "167.95 µs"],
          ["kuzu", "234.56 µs", 1.4],
          ["grafeo", "226.74 µs", 1.35],
          ["surrealdb", "5.24 ms", 31.23],
          ["memgraph", "738.63 µs", 4.4],
          ["neo4j", "718.84 µs", 4.28],
          ["helixdb", "3.20 ms", 19.06],
        ],
        "lora",
      ),
    },
    {
      name: "order_by_multi_key",
      size: 1000,
      ...W(
        [
          ["lora", "211.99 µs"],
          ["kuzu", "276.69 µs", 1.31],
          ["grafeo", "442.71 µs", 2.09],
          ["memgraph", "871.82 µs", 4.11],
          ["neo4j", "554.84 µs", 2.62],
          ["helixdb", "2.80 ms", 13.19],
        ],
        "lora",
      ),
    },
    {
      name: "skip_limit",
      size: 1000,
      ...W(
        [
          ["lora", "162.19 µs"],
          ["kuzu", "251.28 µs", 1.55],
          ["grafeo", "233.92 µs", 1.44],
          ["surrealdb", "5.24 ms", 32.3],
          ["memgraph", "704.66 µs", 4.34],
          ["neo4j", "612.55 µs", 3.78],
          ["helixdb", "2.87 ms", 17.67],
        ],
        "lora",
      ),
    },
  ],
  traversals: [
    {
      name: "direct_record_traversal",
      size: 500,
      ...W(
        [
          ["lora", "831.70 ns"],
          ["kuzu", "255.86 µs", 307.64],
          ["grafeo", "64.09 µs", 77.06],
          ["surrealdb", "62.98 µs", 75.73],
          ["memgraph", "384.41 µs", 462.2],
          ["neo4j", "616.93 µs", 741.77],
          ["helixdb", "1.55 ms", 1860.62],
        ],
        "lora",
      ),
    },
    {
      name: "recursive_depth2",
      size: 500,
      ...W(
        [
          ["lora", "968.69 ns"],
          ["kuzu", "499.89 µs", 516.05],
          ["grafeo", "62.29 µs", 64.3],
          ["surrealdb", "110.22 µs", 113.79],
          ["memgraph", "380.49 µs", 392.78],
          ["neo4j", "601.14 µs", 620.57],
          ["helixdb", "1.69 ms", 1741.68],
        ],
        "lora",
      ),
    },
    {
      name: "recursive_depth3",
      size: 500,
      ...W(
        [
          ["lora", "1.04 µs"],
          ["kuzu", "540.33 µs", 519.88],
          ["grafeo", "62.87 µs", 60.49],
          ["surrealdb", "147.78 µs", 142.19],
          ["memgraph", "382.45 µs", 367.97],
          ["neo4j", "592.76 µs", 570.32],
          ["helixdb", "1.46 ms", 1401.95],
        ],
        "lora",
      ),
    },
    {
      name: "recursive_depth5",
      size: 500,
      ...W(
        [
          ["lora", "1.16 µs"],
          ["kuzu", "511.05 µs", 440.16],
          ["grafeo", "62.30 µs", 53.66],
          ["surrealdb", "225.94 µs", 194.59],
          ["memgraph", "426.52 µs", 367.35],
          ["neo4j", "606.31 µs", 522.2],
          ["helixdb", "1.65 ms", 1424.57],
        ],
        "lora",
      ),
    },
    {
      name: "relation_filter",
      size: 500,
      ...W(
        [
          ["lora", "117.78 µs"],
          ["kuzu", "406.36 µs", 3.45],
          ["surrealdb", "18.70 ms", 158.81],
          ["memgraph", "868.53 µs", 7.37],
          ["neo4j", "586.82 µs", 4.98],
          ["helixdb", "2.50 ms", 21.19],
        ],
        "lora",
      ),
    },
    {
      name: "traversal_count_one_hop",
      size: 500,
      ...W(
        [
          ["lora", "59.36 µs"],
          ["kuzu", "288.00 µs", 4.85],
          ["grafeo", "141.39 µs", 2.38],
          ["surrealdb", "106.91 µs", 1.8],
          ["memgraph", "394.49 µs", 6.65],
          ["neo4j", "609.51 µs", 10.27],
        ],
        "lora",
      ),
    },
    {
      name: "traversal_filter_one_hop",
      size: 500,
      ...W(
        [
          ["lora", "138.99 µs"],
          ["kuzu", "405.25 µs", 2.92],
          ["grafeo", "259.07 µs", 1.86],
          ["surrealdb", "21.51 ms", 154.73],
          ["memgraph", "1.04 ms", 7.49],
          ["neo4j", "582.58 µs", 4.19],
          ["helixdb", "3.15 ms", 22.66],
        ],
        "lora",
      ),
    },
    {
      name: "traversal_one_hop",
      size: 500,
      ...W(
        [
          ["lora", "125.98 µs"],
          ["kuzu", "360.18 µs", 2.86],
          ["grafeo", "263.65 µs", 2.09],
          ["surrealdb", "25.78 ms", 204.62],
          ["memgraph", "1.14 ms", 9.06],
          ["neo4j", "570.56 µs", 4.53],
          ["helixdb", "2.59 ms", 20.53],
        ],
        "lora",
      ),
    },
    {
      name: "traversal_reverse",
      size: 500,
      ...W(
        [
          ["lora", "123.67 µs"],
          ["kuzu", "365.12 µs", 2.95],
          ["grafeo", "266.69 µs", 2.16],
          ["surrealdb", "24.71 ms", 199.82],
          ["memgraph", "1.10 ms", 8.93],
          ["neo4j", "591.25 µs", 4.78],
          ["helixdb", "2.62 ms", 21.16],
        ],
        "lora",
      ),
    },
    {
      name: "traversal_three_hop",
      size: 500,
      ...W(
        [
          ["lora", "236.10 µs"],
          ["kuzu", "1.13 ms", 4.8],
          ["grafeo", "556.10 µs", 2.36],
          ["surrealdb", "60.59 ms", 256.63],
          ["memgraph", "1.23 ms", 5.21],
          ["neo4j", "607.69 µs", 2.57],
          ["helixdb", "5.16 ms", 21.86],
        ],
        "lora",
      ),
    },
    {
      name: "traversal_two_hop",
      size: 500,
      ...W(
        [
          ["lora", "165.58 µs"],
          ["kuzu", "649.69 µs", 3.92],
          ["grafeo", "411.83 µs", 2.49],
          ["surrealdb", "44.18 ms", 266.85],
          ["memgraph", "1.27 ms", 7.66],
          ["neo4j", "555.27 µs", 3.35],
          ["helixdb", "64.61 ms", 390.24],
        ],
        "lora",
      ),
    },
    {
      name: "traversal_undirected",
      size: 500,
      ...W(
        [
          ["lora", "213.41 µs"],
          ["kuzu", "492.51 µs", 2.31],
          ["grafeo", "494.10 µs", 2.32],
          ["memgraph", "1.76 ms", 8.26],
          ["neo4j", "637.72 µs", 2.99],
          ["helixdb", "4.20 ms", 19.66],
        ],
        "lora",
      ),
    },
    {
      name: "variable_length_path",
      size: 100,
      ...W(
        [
          ["lora", "86.24 µs"],
          ["kuzu", "2.64 ms", 30.62],
          ["grafeo", "210.43 µs", 2.44],
          ["memgraph", "809.87 µs", 9.39],
          ["neo4j", "605.82 µs", 7.02],
        ],
        "lora",
      ),
    },
    {
      name: "varlen_2_to_5",
      size: 100,
      ...W(
        [
          ["lora", "123.74 µs"],
          ["kuzu", "3.90 ms", 31.5],
          ["grafeo", "274.33 µs", 2.22],
          ["memgraph", "986.89 µs", 7.98],
          ["neo4j", "578.29 µs", 4.67],
        ],
        "lora",
      ),
    },
    {
      name: "varlen_exact_5",
      size: 100,
      ...W(
        [
          ["lora", "56.89 µs"],
          ["kuzu", "3.86 ms", 67.92],
          ["grafeo", "140.52 µs", 2.47],
          ["memgraph", "576.13 µs", 10.13],
          ["neo4j", "586.35 µs", 10.31],
        ],
        "lora",
      ),
    },
  ],
  patterns: [
    {
      name: "edge_subquery_clause",
      size: 500,
      ...W(
        [
          ["lora", "213.55 µs"],
          ["kuzu", "465.09 µs", 2.18],
          ["surrealdb", "18.19 ms", 85.2],
          ["memgraph", "1.18 ms", 5.52],
          ["neo4j", "605.92 µs", 2.84],
          ["helixdb", "4.12 ms", 19.29],
        ],
        "lora",
      ),
    },
    {
      name: "star_fanout",
      size: 1000,
      ...W(
        [
          ["lora", "138.96 µs"],
          ["kuzu", "296.68 µs", 2.13],
          ["grafeo", "321.31 µs", 2.31],
          ["surrealdb", "29.94 ms", 215.43],
          ["memgraph", "1.62 ms", 11.69],
          ["neo4j", "579.41 µs", 4.17],
          ["helixdb", "2.75 ms", 19.79],
        ],
        "lora",
      ),
    },
    {
      name: "star_fanout_count",
      size: 1000,
      ...W(
        [
          ["lora", "61.36 µs"],
          ["kuzu", "290.42 µs", 4.73],
          ["grafeo", "193.38 µs", 3.15],
          ["surrealdb", "20.35 ms", 331.65],
          ["memgraph", "500.19 µs", 8.15],
          ["neo4j", "590.08 µs", 9.62],
        ],
        "lora",
      ),
    },
    {
      name: "star_fanout_filter",
      size: 1000,
      ...W(
        [
          ["lora", "112.53 µs"],
          ["kuzu", "312.37 µs", 2.78],
          ["grafeo", "227.60 µs", 2.02],
          ["surrealdb", "24.13 ms", 214.42],
          ["memgraph", "707.87 µs", 6.29],
          ["neo4j", "599.26 µs", 5.33],
          ["helixdb", "3.24 ms", 28.82],
        ],
        "lora",
      ),
    },
  ],
};

// Notes / omissions, keyed `${group}.${workload}.${engine}`.
// `kind: "omitted"` is the only flavour today, but the schema is open
// in case a future report adds e.g. cache-warm vs cold notes.
export const NOTES = [
  // writes
  {
    group: "writes",
    workload: "write_single",
    engine: "kuzu",
    kind: "omitted",
    body: "Kuzu requires `CREATE NODE TABLE` before inserts; the empty fixture has no schema and adding one would change the iteration's measured cost.",
  },
  {
    group: "writes",
    workload: "merge_existing",
    engine: "helixdb",
    kind: "omitted",
    body: "HelixDB has no MERGE/upsert; emulating it needs conditional var_as_if branching.",
  },
  {
    group: "writes",
    workload: "merge_existing",
    engine: "surrealdb",
    kind: "omitted",
    body: "SurrealDB's UPSERT semantics diverge from Cypher MERGE on which fields are matched vs set.",
  },
  {
    group: "writes",
    workload: "merge_create",
    engine: "helixdb",
    kind: "omitted",
    body: "HelixDB has no MERGE/upsert (see merge_existing).",
  },
  {
    group: "writes",
    workload: "merge_create",
    engine: "surrealdb",
    kind: "omitted",
    body: "See merge_existing — UPSERT semantics differ.",
  },
  {
    group: "writes",
    workload: "bulk_edges",
    engine: "surrealdb",
    kind: "omitted",
    body: "UNWIND-driven bulk RELATE requires scripted FOR loops; not a like-for-like comparison.",
  },

  // scans
  {
    group: "scans",
    workload: "distinct",
    engine: "helixdb",
    kind: "omitted",
    body: "HelixDB's dedup is node-level; there is no SELECT DISTINCT <property>.",
  },
  {
    group: "scans",
    workload: "distinct",
    engine: "surrealdb",
    kind: "omitted",
    body: "`value` is a reserved word in SurrealQL's SELECT VALUE clause; no clean equivalent.",
  },

  // strings — helix has no scalar string fns
  {
    group: "strings",
    workload: "string_to_upper",
    engine: "helixdb",
    kind: "omitted",
    body: "HelixDB's DSL has no scalar string functions (upper/lower/substring/length/concat) — only property filters and graph traversal.",
  },
  {
    group: "strings",
    workload: "string_to_lower",
    engine: "helixdb",
    kind: "omitted",
    body: "No scalar string functions in the DSL (see string_to_upper).",
  },
  {
    group: "strings",
    workload: "string_substring",
    engine: "helixdb",
    kind: "omitted",
    body: "No scalar string functions in the DSL (see string_to_upper).",
  },
  {
    group: "strings",
    workload: "string_size",
    engine: "helixdb",
    kind: "omitted",
    body: "No scalar string functions in the DSL (see string_to_upper).",
  },
  {
    group: "strings",
    workload: "string_concat",
    engine: "helixdb",
    kind: "omitted",
    body: "No scalar string functions in the DSL (see string_to_upper).",
  },

  // numerics
  {
    group: "numerics",
    workload: "numeric_abs",
    engine: "helixdb",
    kind: "omitted",
    body: "HelixDB's Expr supports +,-,*,/,% but no abs/floor/ceil/round.",
  },
  {
    group: "numerics",
    workload: "numeric_modulo",
    engine: "surrealdb",
    kind: "omitted",
    body: "SurrealQL parser rejects bare `%` inside SELECT projections (same parse limit as grouped_aggregation).",
  },
  {
    group: "numerics",
    workload: "numeric_floor",
    engine: "helixdb",
    kind: "omitted",
    body: "HelixDB's Expr supports +,-,*,/,% but no abs/floor/ceil/round.",
  },
  {
    group: "numerics",
    workload: "numeric_ceil",
    engine: "helixdb",
    kind: "omitted",
    body: "HelixDB's Expr supports +,-,*,/,% but no abs/floor/ceil/round.",
  },
  {
    group: "numerics",
    workload: "numeric_round",
    engine: "helixdb",
    kind: "omitted",
    body: "HelixDB's Expr supports +,-,*,/,% but no abs/floor/ceil/round.",
  },

  // aggregates
  {
    group: "aggregates",
    workload: "aggregate_count",
    engine: "helixdb",
    kind: "omitted",
    body: "The HelixDB enterprise-dev image rejects `count()` dynamic queries with `rate limit exceeded` (its other 9 workloads run fine). The count handler is wired in helixdb.rs and would run on a server without that limit.",
  },
  {
    group: "aggregates",
    workload: "aggregate_collect",
    engine: "helixdb",
    kind: "omitted",
    body: "aggregate_by offers Count/Sum/Min/Max/Mean, no list collect.",
  },
  {
    group: "aggregates",
    workload: "aggregate_count_distinct",
    engine: "helixdb",
    kind: "omitted",
    body: "Needs count(DISTINCT); count() is rate-limited on the enterprise-dev image and value-level DISTINCT isn't exposed.",
  },
  {
    group: "aggregates",
    workload: "aggregate_count_distinct",
    engine: "surrealdb",
    kind: "omitted",
    body: "count(DISTINCT) has no direct SurrealQL aggregate; requires nested SELECT + array::distinct.",
  },
  {
    group: "aggregates",
    workload: "grouped_aggregation",
    engine: "helixdb",
    kind: "omitted",
    body: "group_count groups by a stored property, not a computed value % 10 key.",
  },
  {
    group: "aggregates",
    workload: "grouped_aggregation",
    engine: "surrealdb",
    kind: "omitted",
    body: "SurrealQL rejects `%` inside a SELECT projection that's then used as a GROUP BY key (parse error).",
  },

  // pipeline
  {
    group: "pipeline",
    workload: "with_pipeline",
    engine: "helixdb",
    kind: "omitted",
    body: "Returns count(...); count() is rate-limited on the enterprise-dev image (see aggregate_count).",
  },
  {
    group: "pipeline",
    workload: "with_distinct_then_count",
    engine: "helixdb",
    kind: "omitted",
    body: "count() rate-limited and no value-level DISTINCT.",
  },
  {
    group: "pipeline",
    workload: "with_distinct_then_count",
    engine: "surrealdb",
    kind: "omitted",
    body: "DISTINCT on `value` needs SELECT VALUE, where `value` is reserved (same parse limit as the `distinct` workload).",
  },
  {
    group: "pipeline",
    workload: "with_aggregate_then_filter",
    engine: "helixdb",
    kind: "omitted",
    body: "Group-then-having on a computed key (value % 10) isn't expressible; group_count keys on a stored property.",
  },
  {
    group: "pipeline",
    workload: "with_aggregate_then_filter",
    engine: "surrealdb",
    kind: "omitted",
    body: "Groups on `value % 10`; SurrealQL rejects `%` in a projection used as a GROUP BY key (same parse limit as grouped_aggregation).",
  },
  {
    group: "pipeline",
    workload: "predicate_via_function",
    engine: "helixdb",
    kind: "omitted",
    body: "WHERE size(name) needs a length() scalar the DSL doesn't provide.",
  },
  {
    group: "pipeline",
    workload: "distinct_with_order",
    engine: "helixdb",
    kind: "omitted",
    body: "No value-level DISTINCT (see distinct).",
  },
  {
    group: "pipeline",
    workload: "distinct_with_order",
    engine: "surrealdb",
    kind: "omitted",
    body: "DISTINCT + ORDER BY on `value`, which is reserved in both SELECT VALUE and ORDER BY (see distinct, order_by_multi_key).",
  },

  // lists
  {
    group: "lists",
    workload: "range_function",
    engine: "surrealdb",
    kind: "omitted",
    body: "SurrealQL has no row-generating numeric range (no UNWIND/range equivalent); the explicit-list unwind is covered by list_unwind_explicit.",
  },

  // sort
  {
    group: "sort",
    workload: "order_by_multi_key",
    engine: "surrealdb",
    kind: "omitted",
    body: "`value` is reserved in SurrealQL ORDER BY clauses (parse error).",
  },

  // traversals
  {
    group: "traversals",
    workload: "traversal_undirected",
    engine: "surrealdb",
    kind: "omitted",
    body: "SurrealDB graph edges are directional; the undirected `-[:NEXT]-` pattern has no single like-for-like arrow form (forward and reverse are covered by traversal_one_hop and traversal_reverse).",
  },
  {
    group: "traversals",
    workload: "variable_length_path",
    engine: "helixdb",
    kind: "omitted",
    body: "Range-bounded variable-length expansion isn't mapped; fixed depths are benched as recursive_depth2/3/5.",
  },
  {
    group: "traversals",
    workload: "variable_length_path",
    engine: "surrealdb",
    kind: "omitted",
    body: "SurrealQL recursive traversal takes a fixed depth `@{n}` (see recursive_depth2/3/5); a `1..3` range expanded from every start node has no like-for-like form.",
  },
  {
    group: "traversals",
    workload: "varlen_2_to_5",
    engine: "helixdb",
    kind: "omitted",
    body: "Range-bounded variable-length expansion isn't mapped (see variable_length_path).",
  },
  {
    group: "traversals",
    workload: "varlen_2_to_5",
    engine: "surrealdb",
    kind: "omitted",
    body: "Range-bounded variable-length expansion from every start node; see variable_length_path.",
  },
  {
    group: "traversals",
    workload: "varlen_exact_5",
    engine: "helixdb",
    kind: "omitted",
    body: "Range-bounded variable-length expansion isn't mapped (see variable_length_path).",
  },
  {
    group: "traversals",
    workload: "varlen_exact_5",
    engine: "surrealdb",
    kind: "omitted",
    body: "Fixed-depth expansion from every start node; the anchored single-source equivalent is benched as recursive_depth5.",
  },
  {
    group: "traversals",
    workload: "relation_filter",
    engine: "grafeo",
    kind: "omitted",
    body: "grafeo's `create_edge` facade takes no edge properties, so the chain fixture has no `step` to filter on. memgraph/neo4j/kuzu seed `step` directly, so they do run this workload.",
  },

  // patterns
  {
    group: "patterns",
    workload: "star_fanout_count",
    engine: "helixdb",
    kind: "omitted",
    body: "The HelixDB enterprise-dev image rejects `count()` dynamic queries with `rate limit exceeded` (its other 9 workloads run fine); see aggregate_count.",
  },
  {
    group: "patterns",
    workload: "edge_subquery_clause",
    engine: "grafeo",
    kind: "omitted",
    body: "grafeo's `create_edge` facade takes no edge properties, so the social fixture has no `strength` to filter on. memgraph/neo4j/kuzu seed `strength` directly, so they do run this workload.",
  },
];
