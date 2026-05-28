use serde::Deserialize;

/// The shape of seed data a workload runs on.
///
/// Each engine module is responsible for materialising the same logical
/// fixture in its own storage. The runner only knows the fixture *kind*
/// and *size* — it never inspects the data itself.
#[derive(Debug, Deserialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum FixtureKind {
    /// No seed data — used by construct, write_single, and write_bulk.
    Empty,
    /// `n` `:Node {id, name, value}` rows. The default scratch dataset.
    Nodes,
    /// `n` `:Chain {idx}` nodes joined by `:NEXT {step}` edges in a line.
    Chain,
    /// 1 `:Hub` + `n` `:Leaf {id}` joined by `:ARM` edges.
    Star,
    /// `n` `:Person {idx, name}` with two outgoing `:KNOWS {strength}` edges.
    Social,
}

/// A fully-resolved fixture: kind plus the size to seed.
#[derive(Debug, Clone, Copy)]
pub struct Fixture {
    pub kind: FixtureKind,
    pub size: usize,
}
