//! Re-export of the builtin metadata table.
//!
//! The actual specs and aliases live in the `lora-builtins-meta` crate
//! so the editor's WASM bridge can pull from the same source of truth
//! without dragging in `lora-store` (an analyzer transitive dep). This
//! shim keeps the existing in-tree call sites working unchanged.
pub use lora_builtins_meta::*;
