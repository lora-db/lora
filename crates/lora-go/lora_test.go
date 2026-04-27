package lora_test

import (
	"context"
	"errors"
	"fmt"
	"math"
	"sync"
	"testing"

	lora "github.com/lora-db/lora/crates/lora-go"
)

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

func newDB(t *testing.T) *lora.Database {
	t.Helper()
	db, err := lora.New()
	if err != nil {
		t.Fatalf("lora.New: %v", err)
	}
	t.Cleanup(func() { _ = db.Close() })
	return db
}

func mustExec(t *testing.T, db *lora.Database, q string, p lora.Params) *lora.Result {
	t.Helper()
	r, err := db.Execute(q, p)
	if err != nil {
		t.Fatalf("execute %q: %v", q, err)
	}
	return r
}

func rowAt(t *testing.T, r *lora.Result, i int) lora.Row {
	t.Helper()
	if len(r.Rows) <= i {
		t.Fatalf("expected at least %d rows, got %d", i+1, len(r.Rows))
	}
	return r.Rows[i]
}

// ---------------------------------------------------------------------------
// Baseline: the engine is wired up.
// ---------------------------------------------------------------------------

func TestVersionIsNotEmpty(t *testing.T) {
	if v := lora.Version(); v == "" {
		t.Fatal("Version returned empty string")
	}
}

func TestEmptyMatchReturnsEmptyRows(t *testing.T) {
	db := newDB(t)
	r := mustExec(t, db, "MATCH (n) RETURN n", nil)
	if len(r.Rows) != 0 {
		t.Fatalf("expected zero rows, got %#v", r.Rows)
	}
}

// ---------------------------------------------------------------------------
// Create / match / counts
// ---------------------------------------------------------------------------

func TestCreateAndReturnNodeWithProperties(t *testing.T) {
	db := newDB(t)
	mustExec(t, db, "CREATE (:Person {name: 'Alice', age: 30})", nil)

	n, err := db.NodeCount()
	if err != nil {
		t.Fatal(err)
	}
	if n != 1 {
		t.Fatalf("NodeCount = %d, want 1", n)
	}

	r := mustExec(t, db, "MATCH (n:Person) RETURN n", nil)
	row := rowAt(t, r, 0)
	if !lora.IsNode(row["n"]) {
		t.Fatalf("expected node, got %#v", row["n"])
	}
	node := row["n"].(map[string]any)
	if labels, _ := node["labels"].([]any); len(labels) != 1 || labels[0] != "Person" {
		t.Fatalf("labels = %#v, want [Person]", labels)
	}
	props := node["properties"].(map[string]any)
	if props["name"] != "Alice" {
		t.Fatalf("name = %#v", props["name"])
	}
	if props["age"] != int64(30) {
		t.Fatalf("age = %#v (type %T)", props["age"], props["age"])
	}
}

func TestStreamAndTransactionHelpers(t *testing.T) {
	db := newDB(t)
	results, err := db.Transaction([]lora.TransactionStatement{
		{Query: "UNWIND range(1, 3) AS i CREATE (:S {i: i})"},
		{Query: "MATCH (n:S) RETURN n.i AS i ORDER BY i"},
	}, lora.TransactionReadWrite)
	if err != nil {
		t.Fatalf("Transaction: %v", err)
	}
	if got := results[1].Rows[0]["i"]; got != int64(1) {
		t.Fatalf("transaction row = %#v", got)
	}

	it, err := db.Stream("MATCH (n:S) RETURN n.i AS i ORDER BY i", nil)
	if err != nil {
		t.Fatalf("Stream: %v", err)
	}
	defer it.Close()
	cols, err := it.Columns()
	if err != nil {
		t.Fatalf("Columns: %v", err)
	}
	if fmt.Sprint(cols) != "[i]" {
		t.Fatalf("columns = %v", cols)
	}
	var seen []int64
	for it.Next() {
		seen = append(seen, it.Row()["i"].(int64))
	}
	if err := it.Err(); err != nil {
		t.Fatalf("stream err: %v", err)
	}
	if fmt.Sprint(seen) != "[1 2 3]" {
		t.Fatalf("stream rows = %v", seen)
	}

	early, err := db.Stream("UNWIND range(1, 3) AS i CREATE (:EarlyClose {i: i}) RETURN i", nil)
	if err != nil {
		t.Fatalf("early Stream: %v", err)
	}
	if !early.Next() {
		t.Fatalf("expected first early row, err=%v", early.Err())
	}
	if got := early.Row()["i"]; got != int64(1) {
		t.Fatalf("early row = %#v", got)
	}
	if err := early.Close(); err != nil {
		t.Fatalf("early Close: %v", err)
	}
	nc, err := db.NodeCount()
	if err != nil {
		t.Fatal(err)
	}
	if nc != 3 {
		t.Fatalf("early close did not rollback; node count = %d", nc)
	}

	_, err = db.Transaction([]lora.TransactionStatement{
		{Query: "CREATE (:S {i: 99})"},
		{Query: "THIS IS NOT CYPHER"},
	}, lora.TransactionReadWrite)
	if err == nil {
		t.Fatal("expected transaction error")
	}
	r := mustExec(t, db, "MATCH (n:S) RETURN n.i AS i ORDER BY i", nil)
	if len(r.Rows) != 3 {
		t.Fatalf("rollback failed; rows = %#v", r.Rows)
	}
}

func TestClearEmptiesGraphAndCounts(t *testing.T) {
	db := newDB(t)
	mustExec(t, db, "CREATE (:X), (:Y)-[:R]->(:Z)", nil)

	nc, err := db.NodeCount()
	if err != nil {
		t.Fatal(err)
	}
	rc, err := db.RelationshipCount()
	if err != nil {
		t.Fatal(err)
	}
	if nc != 3 || rc != 1 {
		t.Fatalf("pre-clear counts nc=%d rc=%d", nc, rc)
	}

	if err := db.Clear(); err != nil {
		t.Fatalf("Clear: %v", err)
	}
	nc, _ = db.NodeCount()
	rc, _ = db.RelationshipCount()
	if nc != 0 || rc != 0 {
		t.Fatalf("post-clear counts nc=%d rc=%d", nc, rc)
	}
}

// ---------------------------------------------------------------------------
// Scalar, list, map params
// ---------------------------------------------------------------------------

func TestScalarParamsRoundTrip(t *testing.T) {
	db := newDB(t)
	mustExec(t, db,
		"CREATE (:Item {name: $n, qty: $q, active: $a, score: $s, missing: $m})",
		lora.Params{"n": "widget", "q": 42, "a": true, "s": 1.5, "m": nil},
	)
	r := mustExec(t, db,
		"MATCH (i:Item) RETURN i.name AS name, i.qty AS qty, i.active AS active, i.score AS score, i.missing AS missing",
		nil,
	)
	row := rowAt(t, r, 0)
	if row["name"] != "widget" {
		t.Fatalf("name = %#v", row["name"])
	}
	if row["qty"] != int64(42) {
		t.Fatalf("qty = %#v (type %T)", row["qty"], row["qty"])
	}
	if row["active"] != true {
		t.Fatalf("active = %#v", row["active"])
	}
	// json: 1.5 is representable exactly as float64
	if row["score"] != 1.5 {
		t.Fatalf("score = %#v (type %T)", row["score"], row["score"])
	}
	if row["missing"] != nil {
		t.Fatalf("missing = %#v", row["missing"])
	}
}

func TestNestedListRoundTrip(t *testing.T) {
	db := newDB(t)
	mustExec(t, db, "CREATE (:N {xs: $xs})", lora.Params{
		"xs": []any{int64(1), "two", true, nil, []any{int64(2), int64(3)}},
	})
	rows := mustExec(t, db, "MATCH (n:N) RETURN n.xs AS xs", nil).Rows
	got := rows[0]["xs"].([]any)
	want := []any{int64(1), "two", true, nil, []any{int64(2), int64(3)}}
	if !sliceEqual(got, want) {
		t.Fatalf("xs = %#v, want %#v", got, want)
	}
}

func TestNestedMapRoundTrip(t *testing.T) {
	db := newDB(t)
	mustExec(t, db, "CREATE (:N {meta: $m})", lora.Params{
		"m": map[string]any{
			"a": int64(1),
			"b": map[string]any{
				"c": "deep",
				"d": []any{true, false},
			},
		},
	})
	rows := mustExec(t, db, "MATCH (n:N) RETURN n.meta AS m", nil).Rows
	m := rows[0]["m"].(map[string]any)
	if m["a"] != int64(1) {
		t.Fatalf("m.a = %#v", m["a"])
	}
	inner := m["b"].(map[string]any)
	if inner["c"] != "deep" {
		t.Fatalf("m.b.c = %#v", inner["c"])
	}
	if !sliceEqual(inner["d"].([]any), []any{true, false}) {
		t.Fatalf("m.b.d = %#v", inner["d"])
	}
}

// ---------------------------------------------------------------------------
// Relationships + paths
// ---------------------------------------------------------------------------

func TestRelationshipHasDiscriminator(t *testing.T) {
	db := newDB(t)
	mustExec(t, db, "CREATE (:A {n:1})-[:R {w:2}]->(:B {n:3})", nil)
	rows := mustExec(t, db, "MATCH ()-[r:R]->() RETURN r", nil).Rows
	rel := rows[0]["r"]
	if !lora.IsRelationship(rel) {
		t.Fatalf("expected relationship, got %#v", rel)
	}
}

func TestPathInvariant(t *testing.T) {
	db := newDB(t)
	mustExec(t, db, "CREATE (:A {n:1})-[:R]->(:B {n:2})", nil)
	rows := mustExec(t, db, "MATCH p = (:A)-[:R]->(:B) RETURN p", nil).Rows
	p := rows[0]["p"]
	if !lora.IsPath(p) {
		t.Fatalf("expected path, got %#v", p)
	}
	m := p.(map[string]any)
	nodes := m["nodes"].([]any)
	rels := m["rels"].([]any)
	if len(nodes) != len(rels)+1 {
		t.Fatalf("nodes=%d rels=%d (invariant: nodes == rels+1)", len(nodes), len(rels))
	}
}

// ---------------------------------------------------------------------------
// Temporal
// ---------------------------------------------------------------------------

func TestTaggedDateAsCypherLiteral(t *testing.T) {
	db := newDB(t)
	mustExec(t, db, "CREATE (:E {d: date('2025-03-14')})", nil)
	rows := mustExec(t, db, "MATCH (n:E) RETURN n.d AS d", nil).Rows
	d := rows[0]["d"]
	if !lora.IsTemporal(d) {
		t.Fatalf("expected temporal, got %#v", d)
	}
	m := d.(map[string]any)
	if m["kind"] != "date" || m["iso"] != "2025-03-14" {
		t.Fatalf("date = %#v", m)
	}
}

func TestTypedTemporalParams(t *testing.T) {
	db := newDB(t)
	mustExec(t, db,
		"CREATE (:E {on: $d, span: $dur})",
		lora.Params{"d": lora.Date("2025-01-15"), "dur": lora.Duration("P1M")},
	)
	rows := mustExec(t, db, "MATCH (n:E) RETURN n.on AS on, n.span AS span", nil).Rows
	if rows[0]["on"].(map[string]any)["iso"] != "2025-01-15" {
		t.Fatalf("on = %#v", rows[0]["on"])
	}
	if rows[0]["span"].(map[string]any)["iso"] != "P1M" {
		t.Fatalf("span = %#v", rows[0]["span"])
	}
}

func TestTemporalNowFunctions(t *testing.T) {
	db := newDB(t)
	// Every no-arg temporal constructor must return a tagged value
	// and not error out — mirrors lora-python test_temporal_now_functions_work.
	r := mustExec(t, db,
		"RETURN date() AS d, datetime() AS dt, time() AS t, localdatetime() AS ldt, localtime() AS lt",
		nil,
	)
	row := rowAt(t, r, 0)
	for _, k := range []string{"d", "dt", "t", "ldt", "lt"} {
		if !lora.IsTemporal(row[k]) {
			t.Fatalf("%q is not temporal: %#v", k, row[k])
		}
	}
}

// ---------------------------------------------------------------------------
// Spatial
// ---------------------------------------------------------------------------

func TestTaggedPointValues2D(t *testing.T) {
	db := newDB(t)
	mustExec(t, db,
		"CREATE (:P {c: $c, g: $g})",
		lora.Params{"c": lora.Cartesian(1.5, 2.5), "g": lora.WGS84(4.9, 52.37)},
	)
	rows := mustExec(t, db, "MATCH (n:P) RETURN n.c AS c, n.g AS g", nil).Rows
	c := rows[0]["c"].(map[string]any)
	g := rows[0]["g"].(map[string]any)

	if c["srid"] != int64(lora.SRIDCartesian2D) || c["crs"] != "cartesian" {
		t.Fatalf("cartesian srid/crs = %v/%v", c["srid"], c["crs"])
	}
	if !approx(c["x"], 1.5) || !approx(c["y"], 2.5) {
		t.Fatalf("cartesian xy = (%v, %v)", c["x"], c["y"])
	}
	if _, ok := c["z"]; ok {
		t.Fatal("cartesian 2D should not expose z")
	}
	if _, ok := c["longitude"]; ok {
		t.Fatal("cartesian 2D should not expose longitude")
	}

	if g["srid"] != int64(lora.SRIDWGS84_2D) || g["crs"] != "WGS-84-2D" {
		t.Fatalf("wgs84 srid/crs = %v/%v", g["srid"], g["crs"])
	}
	if !approx(g["longitude"], 4.9) || !approx(g["latitude"], 52.37) {
		t.Fatalf("wgs84 geo = (%v, %v)", g["longitude"], g["latitude"])
	}
}

func TestTaggedPointValues3D(t *testing.T) {
	db := newDB(t)
	mustExec(t, db,
		"CREATE (:P3 {c: $c, g: $g})",
		lora.Params{
			"c": lora.Cartesian3D(1.0, 2.0, 3.0),
			"g": lora.WGS84_3D(4.89, 52.37, 15.0),
		},
	)
	rows := mustExec(t, db, "MATCH (n:P3) RETURN n.c AS c, n.g AS g", nil).Rows
	c := rows[0]["c"].(map[string]any)
	g := rows[0]["g"].(map[string]any)

	if c["srid"] != int64(lora.SRIDCartesian3D) || c["crs"] != "cartesian-3D" {
		t.Fatalf("cartesian-3D srid/crs = %v/%v", c["srid"], c["crs"])
	}
	if !approx(c["z"], 3.0) {
		t.Fatalf("cartesian-3D z = %v", c["z"])
	}

	if g["srid"] != int64(lora.SRIDWGS84_3D) || g["crs"] != "WGS-84-3D" {
		t.Fatalf("wgs84-3D srid/crs = %v/%v", g["srid"], g["crs"])
	}
	if !approx(g["height"], 15.0) {
		t.Fatalf("wgs84-3D height = %v", g["height"])
	}
}

func TestPointFromCypherConstructorRoundTrips(t *testing.T) {
	db := newDB(t)
	r := mustExec(t, db, "RETURN point({x: 1.0, y: 2.0, z: 3.0}) AS p", nil)
	p := rowAt(t, r, 0)["p"].(map[string]any)
	if p["srid"] != int64(lora.SRIDCartesian3D) || p["crs"] != "cartesian-3D" {
		t.Fatalf("3D ctor srid/crs = %v/%v", p["srid"], p["crs"])
	}
	if !approx(p["x"], 1.0) || !approx(p["y"], 2.0) || !approx(p["z"], 3.0) {
		t.Fatalf("3D ctor xyz = (%v, %v, %v)", p["x"], p["y"], p["z"])
	}
}

// ---------------------------------------------------------------------------
// Error surfaces
// ---------------------------------------------------------------------------

func TestParseErrorReturnsLoraError(t *testing.T) {
	db := newDB(t)
	_, err := db.Execute("NOT CYPHER", nil)
	var lerr *lora.LoraError
	if !errors.As(err, &lerr) {
		t.Fatalf("expected *LoraError, got %#v", err)
	}
	if lerr.Code != lora.CodeLoraError {
		t.Fatalf("Code = %q, want LORA_ERROR", lerr.Code)
	}
}

func TestInvalidTemporalParamReturnsInvalidParamsError(t *testing.T) {
	db := newDB(t)
	_, err := db.Execute(
		"RETURN $d AS d",
		lora.Params{"d": lora.Date("not-a-date")},
	)
	var lerr *lora.LoraError
	if !errors.As(err, &lerr) {
		t.Fatalf("expected *LoraError, got %#v", err)
	}
	if lerr.Code != lora.CodeInvalidParams {
		t.Fatalf("Code = %q, want INVALID_PARAMS", lerr.Code)
	}
}

func TestErrorsIsMatchesOnCode(t *testing.T) {
	db := newDB(t)
	_, err := db.Execute("NOT CYPHER", nil)
	if !errors.Is(err, &lora.LoraError{Code: lora.CodeLoraError}) {
		t.Fatalf("errors.Is should match on Code, got %v", err)
	}
	if errors.Is(err, &lora.LoraError{Code: lora.CodeInvalidParams}) {
		t.Fatal("errors.Is should NOT cross-match codes")
	}
}

// ---------------------------------------------------------------------------
// Lifecycle
// ---------------------------------------------------------------------------

func TestDoubleCloseIsNoop(t *testing.T) {
	db, err := lora.New()
	if err != nil {
		t.Fatal(err)
	}
	if err := db.Close(); err != nil {
		t.Fatalf("first close: %v", err)
	}
	if err := db.Close(); err != nil {
		t.Fatalf("second close: %v", err)
	}
}

func TestCallAfterCloseIsError(t *testing.T) {
	db, err := lora.New()
	if err != nil {
		t.Fatal(err)
	}
	_ = db.Close()
	if _, err := db.Execute("MATCH (n) RETURN n", nil); err == nil {
		t.Fatal("expected error after Close, got nil")
	}
}

// ---------------------------------------------------------------------------
// Context cancellation caveat
// ---------------------------------------------------------------------------

func TestExecuteContextCancellation(t *testing.T) {
	db := newDB(t)
	ctx, cancel := context.WithCancel(context.Background())
	cancel()
	_, err := db.ExecuteContext(ctx, "MATCH (n) RETURN n", nil)
	if !errors.Is(err, context.Canceled) {
		t.Fatalf("expected context.Canceled, got %v", err)
	}
}

// ---------------------------------------------------------------------------
// Basic concurrency — correctness, not parallelism.
//
// Writes serialize on the Rust-side store lock, so we aren't claiming
// parallel speedup here; we are claiming it is safe to share a single
// *Database across many goroutines and that the results remain
// consistent under `-race`.
// ---------------------------------------------------------------------------

func TestConcurrentReads(t *testing.T) {
	db := newDB(t)
	mustExec(t, db, "CREATE (:N {x:1}), (:N {x:2}), (:N {x:3})", nil)

	const readers = 16
	const perReader = 50

	var wg sync.WaitGroup
	errs := make(chan error, readers)

	for i := 0; i < readers; i++ {
		wg.Add(1)
		go func() {
			defer wg.Done()
			for j := 0; j < perReader; j++ {
				r, err := db.Execute("MATCH (n:N) RETURN count(n) AS c", nil)
				if err != nil {
					errs <- err
					return
				}
				c := r.Rows[0]["c"]
				if c != int64(3) {
					errs <- errTestf("expected count 3, got %#v", c)
					return
				}
			}
		}()
	}

	wg.Wait()
	close(errs)
	for err := range errs {
		t.Error(err)
	}
}

// ---------------------------------------------------------------------------
// Utilities — kept small and local so the test file is self-contained.
// ---------------------------------------------------------------------------

func errTestf(format string, args ...any) error {
	return fmt.Errorf(format, args...)
}

// ---------------------------------------------------------------------------
// Vector
// ---------------------------------------------------------------------------

func TestVectorReturnHasTaggedShape(t *testing.T) {
	db := newDB(t)
	r := mustExec(t, db, "RETURN vector([1,2,3], 3, INTEGER) AS v", nil)
	v, ok := rowAt(t, r, 0)["v"].(map[string]any)
	if !ok {
		t.Fatalf("expected vector map, got %T", rowAt(t, r, 0)["v"])
	}
	if !lora.IsVector(v) {
		t.Fatalf("IsVector returned false")
	}
	dim := v["dimension"]
	if dim != int64(3) && dim != float64(3) {
		t.Fatalf("dimension = %v", dim)
	}
	if v["coordinateType"] != "INTEGER" {
		t.Fatalf("coordinateType = %v", v["coordinateType"])
	}
}

func TestVectorParameterRoundTrip(t *testing.T) {
	db := newDB(t)
	param := lora.Vector(
		[]any{0.1, 0.2, 0.3},
		3,
		lora.VectorCoordTypeFloat32,
	)
	r := mustExec(t, db, "RETURN $v AS v", lora.Params{"v": param})
	v, ok := rowAt(t, r, 0)["v"].(map[string]any)
	if !ok {
		t.Fatalf("expected vector map, got %T", rowAt(t, r, 0)["v"])
	}
	if v["coordinateType"] != "FLOAT32" {
		t.Fatalf("coordinateType = %v", v["coordinateType"])
	}
}

// Vector() also works with a slice literal built from typed numbers (the
// JSON bridge coerces any numeric-looking entry). Documents the public
// API shape: only `[]any` is accepted today — typed slices must be
// converted first.
func TestVectorParameterAcceptsMixedNumericEntries(t *testing.T) {
	db := newDB(t)
	// float64, int64, and int32 all work because each encodes to a JSON
	// number.
	param := lora.Vector(
		[]any{float64(1.5), int64(2), int32(3)},
		3,
		lora.VectorCoordTypeFloat64,
	)
	r := mustExec(t, db, "RETURN $v AS v", lora.Params{"v": param})
	v := rowAt(t, r, 0)["v"].(map[string]any)
	if v["coordinateType"] != "FLOAT64" {
		t.Fatalf("coordinateType = %v", v["coordinateType"])
	}
}

// A vector parameter flows into Cypher's vector.similarity.cosine and
// produces the expected similarity score.
func TestVectorParameterInSimilarityFunction(t *testing.T) {
	db := newDB(t)
	q := lora.Vector(
		[]any{1.0, 0.0, 0.0},
		3,
		lora.VectorCoordTypeFloat32,
	)
	r := mustExec(t,
		db,
		"RETURN vector.similarity.cosine(vector([1.0, 0.0, 0.0], 3, FLOAT32), $q) AS s",
		lora.Params{"q": q},
	)
	s := rowAt(t, r, 0)["s"]
	if !approx(s, 1.0) {
		t.Fatalf("cosine similarity = %v", s)
	}
}

// A malformed vector param (string value instead of number) must return
// the engine's InvalidParams code, matching every other binding.
func TestMalformedVectorParameterReturnsError(t *testing.T) {
	db := newDB(t)
	bad := map[string]any{
		"kind":           "vector",
		"dimension":      2,
		"coordinateType": "FLOAT32",
		"values":         []any{1.0, "oops"},
	}
	_, err := db.Execute("RETURN $v AS v", lora.Params{"v": bad})
	if err == nil {
		t.Fatal("expected an error for malformed vector param")
	}
	var loraErr *lora.LoraError
	if !errors.As(err, &loraErr) {
		t.Fatalf("expected *lora.LoraError, got %T: %v", err, err)
	}
	if loraErr.Code != lora.CodeInvalidParams {
		t.Fatalf("code = %v, want %v", loraErr.Code, lora.CodeInvalidParams)
	}
}

// Missing `dimension` is also InvalidParams — guards the exact tag
// shape clients expect.
func TestMalformedVectorMissingDimensionReturnsError(t *testing.T) {
	db := newDB(t)
	bad := map[string]any{
		"kind":           "vector",
		"coordinateType": "FLOAT32",
		"values":         []any{1.0, 2.0},
	}
	_, err := db.Execute("RETURN $v AS v", lora.Params{"v": bad})
	if err == nil {
		t.Fatal("expected an error for missing dimension")
	}
	var loraErr *lora.LoraError
	if !errors.As(err, &loraErr) {
		t.Fatalf("expected *lora.LoraError, got %T: %v", err, err)
	}
	if loraErr.Code != lora.CodeInvalidParams {
		t.Fatalf("code = %v", loraErr.Code)
	}
}

// IsVector must not accept plain maps, nil, or other tagged values.
func TestIsVectorOnlyAcceptsVectorMaps(t *testing.T) {
	cases := []any{
		nil,
		[]any{1, 2, 3},
		map[string]any{},
		map[string]any{"kind": "node", "id": 1},
		42,
		"vector",
	}
	for _, v := range cases {
		if lora.IsVector(v) {
			t.Errorf("IsVector(%v) should be false", v)
		}
	}
	// Positive control.
	good := map[string]any{
		"kind":           "vector",
		"dimension":      1,
		"coordinateType": "INTEGER",
		"values":         []any{1},
	}
	if !lora.IsVector(good) {
		t.Errorf("IsVector on tagged vector map returned false")
	}
}

// approx returns true when the two numbers are within 1e-9 of each
// other. Used for comparing decoded JSON floats to constants.
func approx(v any, want float64) bool {
	f, ok := v.(float64)
	if !ok {
		if i, ok := v.(int64); ok {
			f = float64(i)
		} else {
			return false
		}
	}
	return math.Abs(f-want) < 1e-9
}

// sliceEqual compares two []any using == on each element, recursing
// into nested []any. Enough for the types the tests use; not a
// general-purpose reflect.DeepEqual replacement.
func sliceEqual(a, b []any) bool {
	if len(a) != len(b) {
		return false
	}
	for i := range a {
		switch ax := a[i].(type) {
		case []any:
			bx, ok := b[i].([]any)
			if !ok || !sliceEqual(ax, bx) {
				return false
			}
		default:
			if a[i] != b[i] {
				return false
			}
		}
	}
	return true
}
