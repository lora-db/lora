use std::collections::BTreeMap;
use std::path::Path;

use serde::Deserialize;

use crate::fixtures::FixtureKind;

/// How Criterion should iterate the workload.
#[derive(Debug, Deserialize, Clone, Copy, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum IterMode {
    /// Seed once outside the timed loop, run the query each iteration.
    /// The default for read-side benches.
    #[default]
    Read,
    /// Fresh seeded DB each iteration, run the query once. Used for
    /// destructive workloads (delete, set-replace) where each iteration
    /// would otherwise mutate state into a different shape.
    PerIterQuery,
    /// Fresh empty DB each iteration; the seed itself is what's measured.
    /// No `query` is run (e.g. `write_bulk`).
    PerIterSeed,
    /// Measure DB construction with no fixture and no query.
    Construct,
}

/// How throughput should be reported for the workload (becomes the
/// `g.throughput(...)` call on the Criterion group).
#[derive(Debug, Deserialize, Clone, Copy, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum ThroughputKind {
    /// No throughput info attached. Default.
    #[default]
    None,
    /// `Throughput::Elements(size)`.
    Elements,
    /// `Throughput::Elements(size - 1)` — chain edge count for one-hop.
    #[serde(rename = "edges_minus_1")]
    EdgesMinus1,
    /// `Throughput::Elements(size - 2)` — two-hop traversal yields n-2 rows.
    #[serde(rename = "edges_minus_2")]
    EdgesMinus2,
    /// `Throughput::Elements(size - 3)` — three-hop.
    #[serde(rename = "edges_minus_3")]
    EdgesMinus3,
    /// `Throughput::Elements(1)` — single-record traversals.
    Single,
}

#[derive(Debug, Deserialize, Clone)]
pub struct Workload {
    /// Stable id used in the criterion group name.
    pub id: String,
    /// Logical group / suite this workload belongs to (e.g. `predicates`).
    pub group: String,
    /// Cross-group category for the website's comparison report
    /// (`graph_traversal`, `aggregation`, `text`, …). Falls back to
    /// `group` when omitted.
    #[serde(default)]
    pub category: Option<String>,
    /// Fixture this workload runs against.
    pub fixture: FixtureKind,
    /// Override the runner's default size for this workload (chain/star
    /// often want smaller sizes than the global `nodes` default).
    #[serde(default)]
    pub size: Option<usize>,
    /// Iteration mode. Defaults to [`IterMode::Read`].
    #[serde(default)]
    pub iter: IterMode,
    /// Engine name → query string. Engines absent from this map are
    /// silently skipped. Supports `${size}` and `${half_size}` placeholders.
    #[serde(default)]
    pub queries: BTreeMap<String, String>,
    /// Engine name → list of property names to index after seeding.
    /// `lora` ignores these (auto-indexes); `grafeo` and `surrealdb`
    /// honour them.
    #[serde(default)]
    pub indexes: BTreeMap<String, Vec<String>>,
    /// Throughput reporting.
    #[serde(default)]
    pub throughput: ThroughputKind,
    /// Engine name → short caveat surfaced in the report (why an engine
    /// is omitted, what to read into a slowdown). Free-form prose.
    #[serde(default)]
    pub notes: BTreeMap<String, String>,
}

#[derive(Debug, Deserialize, Clone, Default)]
pub struct Defaults {
    /// Default size for fixtures that don't specify their own.
    #[serde(default)]
    pub size: Option<usize>,
    /// Per-fixture default size overrides.
    #[serde(default)]
    pub fixture_size: BTreeMap<String, usize>,
    /// Group name → website category. A workload's resolved category is
    /// `workload.category ?? defaults.categories[group] ?? group`.
    #[serde(default)]
    pub categories: BTreeMap<String, String>,
}

#[derive(Debug, Deserialize, Clone)]
pub struct WorkloadFile {
    #[serde(default)]
    pub defaults: Defaults,
    pub workloads: Vec<Workload>,
}

impl WorkloadFile {
    /// Resolved category for a workload. Workload override wins, then
    /// `defaults.categories[group]`, finally the group name itself.
    pub fn category_for(&self, w: &Workload) -> String {
        if let Some(c) = &w.category {
            return c.clone();
        }
        if let Some(c) = self.defaults.categories.get(&w.group) {
            return c.clone();
        }
        w.group.clone()
    }

    /// Cap declared in `defaults.fixture_size` for a fixture (if any).
    /// Workloads bound to the chain/social fixtures often want a smaller
    /// max than the global `defaults.size` to keep traversals tractable.
    pub fn fixture_cap(&self, fixture: FixtureKind) -> Option<usize> {
        let key = match fixture {
            FixtureKind::Empty => "empty",
            FixtureKind::Nodes => "nodes",
            FixtureKind::Chain => "chain",
            FixtureKind::Star => "star",
            FixtureKind::Social => "social",
        };
        self.defaults.fixture_size.get(key).copied()
    }
}

/// Substitute `${size}` and `${half_size}` placeholders in a query.
pub fn substitute(template: &str, size: usize) -> String {
    template
        .replace("${size}", &size.to_string())
        .replace("${half_size}", &(size / 2).to_string())
}

pub fn load_workloads(path: &Path) -> WorkloadFile {
    let yaml = std::fs::read_to_string(path)
        .unwrap_or_else(|e| panic!("failed to read workloads file {path:?}: {e}"));
    serde_yaml::from_str(&yaml).unwrap_or_else(|e| panic!("failed to parse workloads YAML: {e}"))
}
