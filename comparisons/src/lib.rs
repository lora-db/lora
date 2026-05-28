//! Workload-driven comparison harness.
//!
//! Benches are described in `benches/workloads.yml`. Each workload names
//! a fixture, an iteration mode, and a per-engine query string. The
//! `comparison` bench loads the YAML, walks the engine registry, and runs
//! every workload that the engine has a query for.
//!
//! Adding a new engine = create a module under `engines/`, implement the
//! three entry points (`fresh`, `seed`, `execute`), and register it in
//! [`engine::registry`]. Adding a new feature = one YAML entry.

pub mod engine;
pub mod engines;
pub mod fixtures;
pub mod workload;

pub use engine::{registry, DbHandle, EngineSpec};
pub use fixtures::{Fixture, FixtureKind};
pub use workload::{load_workloads, IterMode, ThroughputKind, Workload, WorkloadFile};
