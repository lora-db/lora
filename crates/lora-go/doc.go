// Package lora provides Go bindings for the LoraDB in-memory graph
// database.
//
// The binding links statically against the lora-ffi Rust crate and
// exposes the same execution model as the Node, WASM, and Python
// bindings: one Database handle backed by an in-memory graph store,
// Cypher execution with typed parameters, and row-oriented results
// where structural, temporal, and spatial values carry a "kind"
// discriminator.
//
// # Quick start
//
//	db, err := lora.New()
//	if err != nil {
//	    log.Fatal(err)
//	}
//	defer db.Close()
//
//	_, err = db.Execute(
//	    "CREATE (:Person {name: $n, born: $d})",
//	    lora.Params{"n": "Alice", "d": lora.Date("1990-01-15")},
//	)
//	if err != nil {
//	    log.Fatal(err)
//	}
//
//	r, err := db.Execute("MATCH (p:Person) RETURN p.name AS name, p.born AS born", nil)
//	if err != nil {
//	    log.Fatal(err)
//	}
//	for _, row := range r.Rows {
//	    fmt.Println(row["name"], row["born"])
//	}
//
// # Value model
//
// Input parameters are [Params] (an alias for map[string]any). Scalars
// pass through directly. Typed temporal and spatial values are built
// via the helper constructors ([Date], [Time], [LocalTime], [DateTime],
// [LocalDateTime], [Duration], [Cartesian], [Cartesian3D], [WGS84],
// [WGS84_3D]) — each returns a tagged map the engine recognises.
//
// Returned values follow the same shape: primitives are native Go
// types (bool, int64, float64, string, nil, []any, map[string]any),
// and nodes, relationships, paths, temporal, and spatial values are
// map[string]any with a "kind" field. The [IsNode], [IsRelationship],
// [IsPath], [IsPoint], and [IsTemporal] guards narrow at the call site.
//
// # Context cancellation
//
// [Database.ExecuteContext] cooperates with a context.Context by
// running the native call in a helper goroutine and returning
// ctx.Err() immediately when the context fires. Important caveat:
// the LoraDB executor does not currently support mid-query
// cancellation, so the native call continues running in the
// background and will release the database's internal mutex only
// once it finishes. Follow-up calls that need that mutex will block
// until then.
//
// # Thread safety
//
// A single *Database is safe to share across goroutines. Queries
// serialise on an internal mutex — concurrent executes are correct
// but not parallel. Close must not race with any in-flight call.
package lora
