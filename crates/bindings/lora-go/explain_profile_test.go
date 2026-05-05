package lora_test

import (
	"strings"
	"testing"

	lora "github.com/lora-db/lora/crates/bindings/lora-go"
)

func TestExplainDoesNotExecuteMutatingQuery(t *testing.T) {
	db := newDB(t)
	plan, err := db.Explain("CREATE (:Foo {n: 1})", nil)
	if err != nil {
		t.Fatalf("Explain: %v", err)
	}
	if plan.Shape != lora.PlanShapeMutating {
		t.Fatalf("expected mutating shape, got %q", plan.Shape)
	}
	count, err := db.NodeCount()
	if err != nil {
		t.Fatalf("NodeCount: %v", err)
	}
	if count != 0 {
		t.Fatalf("explain must not mutate; got %d nodes", count)
	}
}

func TestExplainReturnsPlanTree(t *testing.T) {
	db := newDB(t)
	mustExec(t, db, "CREATE (:Person {name: 'Alice'})", nil)
	plan, err := db.Explain("MATCH (p:Person) RETURN p", nil)
	if err != nil {
		t.Fatalf("Explain: %v", err)
	}
	if plan.Shape != lora.PlanShapeReadOnly {
		t.Fatalf("expected readOnly, got %q", plan.Shape)
	}
	if len(plan.ResultColumns) != 1 || plan.ResultColumns[0] != "p" {
		t.Fatalf("unexpected result columns %#v", plan.ResultColumns)
	}
	if plan.Tree.Operator == "" {
		t.Fatalf("expected non-empty root operator")
	}
}

func TestExplainSurfacesParseErrorLikeExecute(t *testing.T) {
	db := newDB(t)
	_, execErr := db.Execute("INVALID", nil)
	if execErr == nil {
		t.Fatal("expected execute to fail on invalid query")
	}
	_, explainErr := db.Explain("INVALID", nil)
	if explainErr == nil {
		t.Fatal("expected explain to fail on invalid query")
	}
	// Both errors share the LORA_ error code prefix.
	if execPrefix(execErr) != execPrefix(explainErr) {
		t.Fatalf("expected matching error prefixes, got %v vs %v", execErr, explainErr)
	}
}

func TestProfileExecutesMutatingQuery(t *testing.T) {
	db := newDB(t)
	prof, err := db.Profile("CREATE (:Foo {n: 1}) RETURN 1 AS one", nil)
	if err != nil {
		t.Fatalf("Profile: %v", err)
	}
	if !prof.Metrics.Mutated {
		t.Fatal("expected mutated=true")
	}
	if prof.Metrics.TotalRows != 1 {
		t.Fatalf("expected 1 row, got %d", prof.Metrics.TotalRows)
	}
	count, _ := db.NodeCount()
	if count != 1 {
		t.Fatalf("profile must persist mutation; got %d nodes", count)
	}
}

func TestProfileReportsPerOperatorTiming(t *testing.T) {
	db := newDB(t)
	for _, name := range []string{"Alice", "Bob", "Carol", "Dave"} {
		mustExec(t, db, "CREATE (:Person {name: '"+name+"'})", nil)
	}
	prof, err := db.Profile(
		"MATCH (p:Person) WHERE p.name <> 'Bob' RETURN p.name AS name",
		nil,
	)
	if err != nil {
		t.Fatalf("Profile: %v", err)
	}
	if prof.Metrics.TotalRows != 3 {
		t.Fatalf("expected 3 rows, got %d", prof.Metrics.TotalRows)
	}
	if len(prof.Metrics.PerOperator) == 0 {
		t.Fatal("expected non-empty per-operator metrics")
	}
	for id, op := range prof.Metrics.PerOperator {
		if op.NextCalls == 0 {
			t.Fatalf("operator %s reported zero next_calls", id)
		}
	}
}

func TestProfileForwardsParams(t *testing.T) {
	db := newDB(t)
	mustExec(t, db, "CREATE (:Person {name: 'Alice'})", nil)
	mustExec(t, db, "CREATE (:Person {name: 'Bob'})", nil)
	prof, err := db.Profile(
		"MATCH (p:Person) WHERE p.name = $name RETURN p",
		lora.Params{"name": "Alice"},
	)
	if err != nil {
		t.Fatalf("Profile: %v", err)
	}
	if prof.Metrics.TotalRows != 1 {
		t.Fatalf("expected 1 row from filtered profile, got %d", prof.Metrics.TotalRows)
	}
}

// execPrefix extracts the leading LORA_* error code from an error
// formatted as "LORA_<code>: <message>".
func execPrefix(err error) string {
	s := err.Error()
	if i := strings.Index(s, ":"); i > 0 {
		return s[:i]
	}
	return s
}
