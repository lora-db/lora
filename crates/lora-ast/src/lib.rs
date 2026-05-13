// AST node variants have deliberate size asymmetry (e.g. a full MATCH/WITH/RETURN
// pipeline vs. a StandaloneCall). Boxing the large variants would trade fewer
// stack copies for an extra heap allocation per parse — the opposite of what
// the parser is tuned for. Self-referential cases that do need indirection
// (e.g. `PatternElement::Parenthesized`) already box explicitly.
#![allow(clippy::large_enum_variant)]

pub mod ast;

pub use ast::{
    BinaryOp, ConstraintKind, ConstraintNameSpec, Create, CreateConstraint, CreateIndex, Delete,
    Direction, Document, DropConstraint, DropIndex, Expr, InQueryCall, IndexEntityKind, IndexKind,
    IndexKindFilter, IndexNameSpec, IndexOptions, ListPredicateKind, LiteralTypeExpr,
    MapProjectionSelector, Match, Merge, MergeAction, MultiPartQuery, NodePattern, Pattern,
    PatternElement, PatternElementChain, PatternPart, ProcedureInvocation, ProcedureInvocationKind,
    ProcedureName, ProjectionBody, ProjectionItem, PropertyTypeExpr, PropertyTypeTerm, Query,
    QueryPart, RangeLiteral, ReadingClause, RegularQuery, RelationshipDetail, RelationshipPattern,
    Remove, RemoveItem, Return, ScalarType, SchemaCommand, Set, SetItem, ShowConstraints,
    ShowIndexes, ShowPipeline, ShowReturn, ShowYield, SinglePartQuery, SingleQuery, SortDirection,
    SortItem, Span, StandaloneCall, Statement, UnaryOp, UnionPart, Unwind, UpdatingClause,
    Variable, VectorCoordType, With, YieldItem,
};
