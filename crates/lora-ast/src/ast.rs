use smallvec::SmallVec;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Span {
    pub start: usize,
    pub end: usize,
}

impl Span {
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
