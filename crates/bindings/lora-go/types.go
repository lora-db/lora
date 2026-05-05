package lora

// Params is the parameter map accepted by [Database.Execute] and
// [Database.ExecuteContext]. Keys are parameter names (without the
// leading `$`); values are Go natives or the tagged maps produced by
// the helpers in helpers.go.
type Params = map[string]any

// Row is a single result row keyed by column name. Values are Go
// natives for primitives and map[string]any for structured values
// (nodes, relationships, paths, points, temporal, vector, and binary values).
// Use the IsNode / IsRelationship / IsPath / IsPoint / IsTemporal guards to
// narrow safely.
type Row = map[string]any

// Result is the shape returned by every successful query. Columns
// lists the projection column names in order; Rows holds one entry per
// matched result, keyed by column name.
type Result struct {
	Columns []string
	Rows    []Row
}

// TransactionStatement is one query in a native transaction batch.
// Results are returned in the same order as the statement slice.
type TransactionStatement struct {
	Query  string `json:"query"`
	Params Params `json:"params,omitempty"`
}

// TransactionMode selects the native transaction mode.
type TransactionMode string

const (
	TransactionReadWrite TransactionMode = "read_write"
	TransactionReadOnly  TransactionMode = "read_only"
)

// PlanShape classifies a compiled plan as read-only or potentially
// mutating. Mirrors lora_executor::StreamShape.
type PlanShape string

const (
	PlanShapeReadOnly PlanShape = "readOnly"
	PlanShapeMutating PlanShape = "mutating"
)

// PlanNode is one operator in a query plan tree. Children are
// leaf-most first under each node; the tree is laid out exactly as
// [Database.Explain] returns it from the native side.
type PlanNode struct {
	ID            uint64            `json:"id"`
	Operator      string            `json:"operator"`
	Details       map[string]string `json:"details"`
	EstimatedRows *uint64           `json:"estimatedRows"`
	Children      []PlanNode        `json:"children"`
}

// QueryPlan is the result of [Database.Explain]. The executor is never
// invoked, so calling Explain on a mutating query is side-effect free.
type QueryPlan struct {
	Query         string    `json:"query"`
	Shape         PlanShape `json:"shape"`
	ResultColumns []string  `json:"resultColumns"`
	Tree          PlanNode  `json:"tree"`
}

// OperatorMetrics is the per-operator runtime metrics block populated
// by [Database.Profile]. Timings are inclusive of children.
type OperatorMetrics struct {
	Rows      uint64 `json:"rows"`
	DbHits    uint64 `json:"dbHits"`
	ElapsedNs uint64 `json:"elapsedNs"`
	NextCalls uint64 `json:"nextCalls"`
}

// ProfileMetrics is the runtime metrics block returned by
// [Database.Profile].
type ProfileMetrics struct {
	TotalElapsedNs uint64                     `json:"totalElapsedNs"`
	TotalRows      uint64                     `json:"totalRows"`
	Mutated        bool                       `json:"mutated"`
	PerOperator    map[string]OperatorMetrics `json:"perOperator"`
}

// QueryProfile is the result of [Database.Profile]. PROFILE executes
// the query for real; mutating queries are persisted exactly as in
// [Database.Execute].
type QueryProfile struct {
	Plan    QueryPlan      `json:"plan"`
	Metrics ProfileMetrics `json:"metrics"`
}
