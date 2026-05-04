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
