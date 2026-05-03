//! Segmented binary value support.
//!
//! Layout:
//!
//! * [`types`] — the [`LoraBinary`] struct + constructor / accessor
//!   surface.
//! * [`traits`] — `PartialEq` / `Eq` / `Hash` / `From<Vec<u8>>` impls,
//!   all rooted in *logical-content* equality so segmented and
//!   contiguous values with the same bytes compare equal.

mod traits;
mod types;

pub use types::LoraBinary;

#[cfg(test)]
mod tests;
