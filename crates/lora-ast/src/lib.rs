// AST node variants have deliberate size asymmetry (e.g. a full MATCH/WITH/RETURN
// pipeline vs. a StandaloneCall). Boxing the large variants would trade fewer
// stack copies for an extra heap allocation per parse — the opposite of what
// the parser is tuned for. Self-referential cases that do need indirection
// (e.g. `PatternElement::Parenthesized`) already box explicitly.
#![allow(clippy::large_enum_variant)]

pub mod ast;

pub use ast::{
    BinaryOp, Create, CreateIndex, Delete, Direction, Document, DropIndex, Expr, InQueryCall,
    IndexEntityKind, IndexKind, IndexNameSpec, IndexOptions, ListPredicateKind,
    MapProjectionSelector, Match, Merge, MergeAction, MultiPartQuery, NodePattern, Pattern,
    PatternElement, PatternElementChain, PatternPart, ProcedureInvocation, ProcedureInvocationKind,
    ProcedureName, ProjectionBody, ProjectionItem, Query, QueryPart, RangeLiteral, ReadingClause,
    RegularQuery, RelationshipDetail, RelationshipPattern, Remove, RemoveItem, Return,
    SchemaCommand, Set, SetItem, ShowIndexes, SinglePartQuery, SingleQuery, SortDirection,
    SortItem, Span, StandaloneCall, Statement, UnaryOp, UnionPart, Unwind, UpdatingClause,
    Variable, With, YieldItem,
};
