// AST node variants have deliberate size asymmetry (e.g. a full MATCH/WITH/RETURN
// pipeline vs. a StandaloneCall). Boxing the large variants would trade fewer
// stack copies for an extra heap allocation per parse — the opposite of what
// the parser is tuned for. Self-referential cases that do need indirection
// (e.g. `PatternElement::Parenthesized`) already box explicitly.
#![allow(clippy::large_enum_variant)]

pub mod ast;
pub use ast::*;
