use smallvec::SmallVec;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Span {
    pub start: usize,
    pub end: usize,
}

impl Span {
    #[must_use]
    pub const fn new(start: usize, end: usize) -> Self {
        Self { start, end }
    }
}

#[derive(Debug, Clone)]
pub struct Document {
    pub statement: Statement,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub enum Statement {
    Query(Query),
    Schema(SchemaCommand),
}

#[derive(Debug, Clone)]
pub enum SchemaCommand {
    CreateIndex(CreateIndex),
    DropIndex(DropIndex),
    ShowIndexes(ShowIndexes),
    CreateConstraint(CreateConstraint),
    DropConstraint(DropConstraint),
    ShowConstraints(ShowConstraints),
}

#[derive(Debug, Clone)]
pub struct CreateConstraint {
    pub name: ConstraintNameSpec,
    pub if_not_exists: bool,
    pub entity: IndexEntityKind,
    /// Pattern variable in the `FOR` clause (`n`, `r`).
    pub variable: String,
    /// Label / rel-type. Always present — constraints don't accept the
    /// wildcard form that LOOKUP indexes do.
    pub label: String,
    pub properties: Vec<String>,
    pub kind: ConstraintKind,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub struct DropConstraint {
    pub name: ConstraintNameSpec,
    pub if_exists: bool,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub struct ShowConstraints {
    pub pipeline: Option<ShowPipeline>,
    pub span: Span,
}

/// `SHOW INDEXES YIELD … [WHERE …] [RETURN …]` tail. Modelled after
/// Cypher-style catalog syntax: YIELD is the anchor, optional WHERE filters the
/// yielded rows, optional RETURN reprojects them. ORDER BY / SKIP /
/// LIMIT can appear on either YIELD or RETURN — semantically applied
/// to the rows at that stage.
#[derive(Debug, Clone)]
pub struct ShowPipeline {
    pub yield_part: ShowYield,
    pub where_: Option<Expr>,
    pub return_part: Option<ShowReturn>,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub struct ShowYield {
    /// `YIELD *` — pass every catalog column through unchanged.
    pub star: bool,
    /// `YIELD a, b AS x` items (empty when `star` is true).
    pub items: Vec<YieldItem>,
    pub order: Vec<SortItem>,
    pub skip: Option<Expr>,
    pub limit: Option<Expr>,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub struct ShowReturn {
    pub items: Vec<ProjectionItem>,
    pub order: Vec<SortItem>,
    pub skip: Option<Expr>,
    pub limit: Option<Expr>,
    pub span: Span,
}

/// Type filter for `SHOW [TYPE] INDEXES`. `All` is the explicit
/// unfiltered form; `Fulltext` and `Vector` parse but yield no rows
/// because those index types aren't backed by anything in the catalog
/// yet.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IndexKindFilter {
    All,
    Range,
    Text,
    Point,
    Lookup,
    Fulltext,
    Vector,
}

#[derive(Debug, Clone)]
pub enum ConstraintNameSpec {
    Literal(String),
    Parameter(String),
}

#[derive(Debug, Clone, PartialEq)]
pub enum ConstraintKind {
    /// `IS UNIQUE` — single or composite.
    Unique,
    /// `IS NOT NULL` — single property only.
    Existence,
    /// `IS NODE KEY` — single or composite, node only.
    NodeKey,
    /// `IS RELATIONSHIP KEY` — single or composite, relationship only.
    RelationshipKey,
    /// `IS :: Type` — single property only.
    PropertyType(PropertyTypeExpr),
}

impl ConstraintKind {
    /// Human-readable tag for SHOW CONSTRAINTS and diagnostics.
    #[must_use]
    pub fn type_tag(&self, entity: IndexEntityKind) -> &'static str {
        match (self, entity) {
            (ConstraintKind::Unique, IndexEntityKind::Node) => "NODE_PROPERTY_UNIQUENESS",
            (ConstraintKind::Unique, IndexEntityKind::Relationship) => {
                "RELATIONSHIP_PROPERTY_UNIQUENESS"
            }
            (ConstraintKind::Existence, IndexEntityKind::Node) => "NODE_PROPERTY_EXISTENCE",
            (ConstraintKind::Existence, IndexEntityKind::Relationship) => {
                "RELATIONSHIP_PROPERTY_EXISTENCE"
            }
            (ConstraintKind::NodeKey, _) => "NODE_KEY",
            (ConstraintKind::RelationshipKey, _) => "RELATIONSHIP_KEY",
            (ConstraintKind::PropertyType(_), IndexEntityKind::Node) => "NODE_PROPERTY_TYPE",
            (ConstraintKind::PropertyType(_), IndexEntityKind::Relationship) => {
                "RELATIONSHIP_PROPERTY_TYPE"
            }
        }
    }
}

/// A closed dynamic union of property types: `T1 | T2 | ...`. A single
/// type is represented as a one-element union.
#[derive(Debug, Clone, PartialEq)]
pub struct PropertyTypeExpr {
    pub alternatives: Vec<PropertyTypeTerm>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum PropertyTypeTerm {
    Scalar(ScalarType),
    List {
        inner: Box<PropertyTypeTerm>,
        /// `LIST<X NOT NULL>` is the only fully-supported list shape in
        /// Cypher compatibility mode; we keep the flag for grammar fidelity
        /// even though we reject `LIST<X>` (nullable elements) at the
        /// catalog layer.
        not_null: bool,
    },
    Vector {
        coord: VectorCoordType,
        dimension: u32,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScalarType {
    Boolean,
    String,
    Integer,
    Float,
    Date,
    LocalTime,
    ZonedTime,
    LocalDateTime,
    ZonedDateTime,
    Duration,
    Point,
    Map,
    Any,
}

impl ScalarType {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            ScalarType::Boolean => "BOOLEAN",
            ScalarType::String => "STRING",
            ScalarType::Integer => "INTEGER",
            ScalarType::Float => "FLOAT",
            ScalarType::Date => "DATE",
            ScalarType::LocalTime => "LOCAL TIME",
            ScalarType::ZonedTime => "ZONED TIME",
            ScalarType::LocalDateTime => "LOCAL DATETIME",
            ScalarType::ZonedDateTime => "ZONED DATETIME",
            ScalarType::Duration => "DURATION",
            ScalarType::Point => "POINT",
            ScalarType::Map => "MAP",
            ScalarType::Any => "ANY",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VectorCoordType {
    Int8,
    Int16,
    Int32,
    Int64,
    Float32,
    Float64,
}

impl VectorCoordType {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            VectorCoordType::Int8 => "INT8",
            VectorCoordType::Int16 => "INT16",
            VectorCoordType::Int32 => "INT32",
            VectorCoordType::Int64 => "INT64",
            VectorCoordType::Float32 => "FLOAT32",
            VectorCoordType::Float64 => "FLOAT64",
        }
    }
}

#[derive(Debug, Clone)]
pub struct DropIndex {
    pub name: IndexNameSpec,
    pub if_exists: bool,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub struct CreateIndex {
    pub kind: IndexKind,
    pub name: Option<IndexNameSpec>,
    pub if_not_exists: bool,
    pub entity: IndexEntityKind,
    /// Pattern variable in the FOR clause (`n` for `(n:Person)`, `r` for `()-[r:KNOWS]-()`).
    pub variable: String,
    /// `Some(label_or_type)` for property indexes; `None` for `LOOKUP` token indexes
    /// where the label/type is the wildcard captured by `labels(n)` / `type(r)`.
    /// For `FULLTEXT` (which accepts `:A|B|C`) this carries the first label; the
    /// rest live in `additional_labels`.
    pub label: Option<String>,
    /// Extra labels beyond `label`. Only populated for `FULLTEXT` indexes
    /// declared with the `(n:A|B|C)` pattern.
    pub additional_labels: Vec<String>,
    /// Property keys covered by the index. Empty for `LOOKUP` token indexes.
    pub properties: Vec<String>,
    pub options: Option<IndexOptions>,
    pub span: Span,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IndexKind {
    Range,
    Text,
    Point,
    Lookup,
    Vector,
    Fulltext,
}

impl IndexKind {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            IndexKind::Range => "RANGE",
            IndexKind::Text => "TEXT",
            IndexKind::Point => "POINT",
            IndexKind::Lookup => "LOOKUP",
            IndexKind::Vector => "VECTOR",
            IndexKind::Fulltext => "FULLTEXT",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IndexEntityKind {
    Node,
    Relationship,
}

impl IndexEntityKind {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            IndexEntityKind::Node => "NODE",
            IndexEntityKind::Relationship => "RELATIONSHIP",
        }
    }
}

#[derive(Debug, Clone)]
pub enum IndexNameSpec {
    Literal(String),
    Parameter(String),
}

#[derive(Debug, Clone)]
pub struct IndexOptions {
    pub config: Vec<(String, Expr)>,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub struct ShowIndexes {
    /// Optional index-type filter (e.g. `SHOW RANGE INDEXES`). `None`
    /// means no filter clause was written; `Some(IndexKindFilter::All)`
    /// is the explicit `SHOW ALL INDEXES` form (semantically the same).
    pub filter: Option<IndexKindFilter>,
    pub pipeline: Option<ShowPipeline>,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub enum Query {
    Regular(RegularQuery),
    StandaloneCall(StandaloneCall),
}

#[derive(Debug, Clone)]
pub struct RegularQuery {
    pub head: SingleQuery,
    pub unions: Vec<UnionPart>,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub struct UnionPart {
    pub all: bool,
    pub query: SingleQuery,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub enum SingleQuery {
    SinglePart(SinglePartQuery),
    MultiPart(MultiPartQuery),
}

#[derive(Debug, Clone)]
pub struct SinglePartQuery {
    pub reading_clauses: Vec<ReadingClause>,
    pub updating_clauses: Vec<UpdatingClause>,
    pub return_clause: Option<Return>,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub struct MultiPartQuery {
    pub parts: Vec<QueryPart>,
    pub tail: Box<SinglePartQuery>,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub struct QueryPart {
    pub reading_clauses: Vec<ReadingClause>,
    pub updating_clauses: Vec<UpdatingClause>,
    pub with_clause: With,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub enum ReadingClause {
    Match(Match),
    Unwind(Unwind),
    InQueryCall(InQueryCall),
}

#[derive(Debug, Clone)]
pub enum UpdatingClause {
    Create(Create),
    Merge(Merge),
    Delete(Delete),
    Set(Set),
    Remove(Remove),
}

#[derive(Debug, Clone)]
pub struct Match {
    pub optional: bool,
    pub pattern: Pattern,
    pub where_: Option<Expr>,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub struct Unwind {
    pub expr: Expr,
    pub alias: Variable,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub struct Create {
    pub pattern: Pattern,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub struct Merge {
    pub pattern_part: PatternPart,
    pub actions: Vec<MergeAction>,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub struct MergeAction {
    pub on_match: bool,
    pub set: Set,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub struct Delete {
    pub detach: bool,
    pub expressions: Vec<Expr>,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub struct Set {
    pub items: Vec<SetItem>,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub enum SetItem {
    SetProperty {
        target: Expr,
        value: Expr,
        span: Span,
    },
    SetVariable {
        variable: Variable,
        value: Expr,
        span: Span,
    },
    MutateVariable {
        variable: Variable,
        value: Expr,
        span: Span,
    },
    SetLabels {
        variable: Variable,
        labels: Vec<String>,
        span: Span,
    },
}

#[derive(Debug, Clone)]
pub struct Remove {
    pub items: Vec<RemoveItem>,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub enum RemoveItem {
    Labels {
        variable: Variable,
        labels: Vec<String>,
        span: Span,
    },
    Property {
        expr: Expr,
        span: Span,
    },
}

#[derive(Debug, Clone)]
pub struct InQueryCall {
    pub procedure: ProcedureInvocation,
    pub yield_items: Vec<YieldItem>,
    pub where_: Option<Expr>,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub struct StandaloneCall {
    pub procedure: ProcedureInvocationKind,
    pub yield_items: Vec<YieldItem>,
    pub yield_all: bool,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub enum ProcedureInvocationKind {
    Explicit(ProcedureInvocation),
    Implicit(ProcedureName),
}

#[derive(Debug, Clone)]
pub struct ProcedureInvocation {
    pub name: ProcedureName,
    pub args: Vec<Expr>,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub struct YieldItem {
    pub field: Option<String>,
    pub alias: Variable,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub struct With {
    pub body: ProjectionBody,
    pub where_: Option<Expr>,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub struct Return {
    pub body: ProjectionBody,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub struct ProjectionBody {
    pub distinct: bool,
    pub items: Vec<ProjectionItem>,
    pub order: Vec<SortItem>,
    pub skip: Option<Expr>,
    pub limit: Option<Expr>,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub enum ProjectionItem {
    Expr {
        expr: Expr,
        alias: Option<Variable>,
        span: Span,
    },
    Star {
        span: Span,
    },
}

#[derive(Debug, Clone)]
pub struct SortItem {
    pub expr: Expr,
    pub direction: SortDirection,
    pub span: Span,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SortDirection {
    Asc,
    Desc,
}

#[derive(Debug, Clone)]
pub struct Pattern {
    pub parts: Vec<PatternPart>,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub struct PatternPart {
    pub binding: Option<Variable>,
    pub element: PatternElement,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub enum PatternElement {
    NodeChain {
        head: NodePattern,
        chain: Vec<PatternElementChain>,
        span: Span,
    },
    Parenthesized(Box<PatternElement>, Span),
    ShortestPath {
        all: bool,
        element: Box<PatternElement>,
        span: Span,
    },
}

#[derive(Debug, Clone)]
pub struct PatternElementChain {
    pub relationship: RelationshipPattern,
    pub node: NodePattern,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub struct NodePattern {
    pub variable: Option<Variable>,
    /// Each inner `Vec` is a disjunctive group (OR).
    /// The outer `SmallVec` is conjunctive (AND across groups).
    /// `:A:B` → `[[A], [B]]`;  `:A|B` → `[[A, B]]`.
    pub labels: SmallVec<SmallVec<String, 2>, 2>,
    pub properties: Option<Expr>,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub struct RelationshipPattern {
    pub direction: Direction,
    pub detail: Option<RelationshipDetail>,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub struct RelationshipDetail {
    pub variable: Option<Variable>,
    pub types: SmallVec<String, 2>,
    pub range: Option<RangeLiteral>,
    pub properties: Option<Expr>,
    pub span: Span,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    Left,
    Right,
    Undirected,
}

#[derive(Debug, Clone)]
pub struct RangeLiteral {
    pub start: Option<u64>,
    pub end: Option<u64>,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub struct Variable {
    pub name: String,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub struct ProcedureName {
    pub parts: Vec<String>,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub enum Expr {
    Variable(Variable),
    Integer(i64, Span),
    Float(f64, Span),
    String(String, Span),
    Bool(bool, Span),
    Null(Span),
    Parameter(String, Span),
    List(Vec<Expr>, Span),
    Map(Vec<(String, Expr)>, Span),
    Property {
        expr: Box<Expr>,
        key: String,
        span: Span,
    },
    Binary {
        lhs: Box<Expr>,
        op: BinaryOp,
        rhs: Box<Expr>,
        span: Span,
    },
    Unary {
        op: UnaryOp,
        expr: Box<Expr>,
        span: Span,
    },
    FunctionCall {
        name: Vec<String>,
        distinct: bool,
        args: Vec<Expr>,
        span: Span,
    },
    TypeCast {
        expr: Box<Expr>,
        target: LiteralTypeExpr,
        try_cast: bool,
        span: Span,
    },
    Case {
        input: Option<Box<Expr>>,
        alternatives: Vec<(Expr, Expr)>,
        else_expr: Option<Box<Expr>>,
        span: Span,
    },
    ListPredicate {
        kind: ListPredicateKind,
        variable: Variable,
        list: Box<Expr>,
        predicate: Box<Expr>,
        span: Span,
    },
    ListComprehension {
        variable: Variable,
        list: Box<Expr>,
        filter: Option<Box<Expr>>,
        map_expr: Option<Box<Expr>>,
        span: Span,
    },
    Reduce {
        accumulator: Variable,
        init: Box<Expr>,
        variable: Variable,
        list: Box<Expr>,
        expr: Box<Expr>,
        span: Span,
    },
    MapProjection {
        base: Box<Expr>,
        selectors: Vec<MapProjectionSelector>,
        span: Span,
    },
    Index {
        expr: Box<Expr>,
        index: Box<Expr>,
        span: Span,
    },
    Slice {
        expr: Box<Expr>,
        from: Option<Box<Expr>>,
        to: Option<Box<Expr>>,
        span: Span,
    },
    ExistsSubquery {
        pattern: Pattern,
        where_: Option<Box<Expr>>,
        span: Span,
    },
    PatternComprehension {
        pattern: Box<PatternElement>,
        where_: Option<Box<Expr>>,
        map_expr: Box<Expr>,
        span: Span,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub enum LiteralTypeExpr {
    Named {
        name: String,
        span: Span,
    },
    List {
        inner: Box<LiteralTypeExpr>,
        span: Span,
    },
    Vector {
        coordinate: String,
        dimension: u32,
        span: Span,
    },
}

impl LiteralTypeExpr {
    #[must_use]
    pub fn span(&self) -> Span {
        match self {
            LiteralTypeExpr::Named { span, .. }
            | LiteralTypeExpr::List { span, .. }
            | LiteralTypeExpr::Vector { span, .. } => *span,
        }
    }
}

#[derive(Debug, Clone)]
pub enum MapProjectionSelector {
    /// `.propertyName` — include a specific property
    Property(String),
    /// `.*` — include all properties
    AllProperties,
    /// `key: expr` — include a computed entry
    Literal(String, Expr),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ListPredicateKind {
    Any,
    All,
    None,
    Single,
}

impl Expr {
    #[must_use]
    pub fn span(&self) -> Span {
        match self {
            Expr::Variable(v) => v.span,
            Expr::Integer(_, s)
            | Expr::Float(_, s)
            | Expr::String(_, s)
            | Expr::Bool(_, s)
            | Expr::Null(s)
            | Expr::Parameter(_, s)
            | Expr::List(_, s)
            | Expr::Map(_, s)
            | Expr::Property { span: s, .. }
            | Expr::Binary { span: s, .. }
            | Expr::Unary { span: s, .. }
            | Expr::FunctionCall { span: s, .. }
            | Expr::TypeCast { span: s, .. }
            | Expr::Case { span: s, .. }
            | Expr::ListPredicate { span: s, .. }
            | Expr::ListComprehension { span: s, .. }
            | Expr::Reduce { span: s, .. }
            | Expr::MapProjection { span: s, .. }
            | Expr::Index { span: s, .. }
            | Expr::Slice { span: s, .. }
            | Expr::ExistsSubquery { span: s, .. }
            | Expr::PatternComprehension { span: s, .. } => *s,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BinaryOp {
    Or,
    Xor,
    And,
    Eq,
    Ne,
    Lt,
    Gt,
    Le,
    Ge,
    Add,
    Sub,
    Mul,
    Div,
    Mod,
    Pow,
    In,
    StartsWith,
    EndsWith,
    Contains,
    IsNull,
    IsNotNull,
    RegexMatch,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnaryOp {
    Not,
    Pos,
    Neg,
}
