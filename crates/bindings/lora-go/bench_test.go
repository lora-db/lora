package lora

import (
	"fmt"
	"testing"
)

// Mirrors the Node binding's bench (`crates/bindings/lora-node/bench/`):
// the same four workloads, so we can read deltas across language
// bindings on a single host.
//
// Run with:
//   go test -bench=. -benchmem -benchtime=1s ./...

func seedNodes(b *testing.B, db *Database, n int) {
	for i := 0; i < n; i++ {
		if _, err := db.Execute(fmt.Sprintf("CREATE (:Node {id: %d, value: %d})", i, i%100), nil); err != nil {
			b.Fatalf("seed failed: %v", err)
		}
	}
}

func seedWide(b *testing.B, db *Database, count, cols int) {
	for i := 0; i < count; i++ {
		props := ""
		for c := 0; c < cols; c++ {
			if c > 0 {
				props += ", "
			}
			props += fmt.Sprintf("p%d: %d", c, i*cols+c)
		}
		if _, err := db.Execute(fmt.Sprintf("CREATE (:Wide {%s})", props), nil); err != nil {
			b.Fatalf("seed failed: %v", err)
		}
	}
}

func seedNested(b *testing.B, db *Database, count, listLen int) {
	for i := 0; i < count; i++ {
		items := ""
		for j := 0; j < listLen; j++ {
			if j > 0 {
				items += ", "
			}
			items += fmt.Sprintf("%d", i*listLen+j)
		}
		if _, err := db.Execute(fmt.Sprintf("CREATE (:Nested {tags: [%s]})", items), nil); err != nil {
			b.Fatalf("seed failed: %v", err)
		}
	}
}

func BenchmarkPointRead10(b *testing.B) {
	db, err := New()
	if err != nil {
		b.Fatal(err)
	}
	defer db.Close()
	seedNodes(b, db, 10_000)
	b.ResetTimer()
	for i := 0; i < b.N; i++ {
		r, err := db.Execute("MATCH (n:Node) WHERE n.id < 10 RETURN n.id, n.value", nil)
		if err != nil {
			b.Fatal(err)
		}
		if len(r.Rows) != 10 {
			b.Fatalf("expected 10 rows, got %d", len(r.Rows))
		}
	}
}

func BenchmarkMediumScan10k(b *testing.B) {
	db, err := New()
	if err != nil {
		b.Fatal(err)
	}
	defer db.Close()
	seedNodes(b, db, 10_000)
	b.ResetTimer()
	for i := 0; i < b.N; i++ {
		r, err := db.Execute("MATCH (n:Node) RETURN n.id", nil)
		if err != nil {
			b.Fatal(err)
		}
		if len(r.Rows) != 10_000 {
			b.Fatalf("expected 10000 rows, got %d", len(r.Rows))
		}
	}
}

func BenchmarkWideRow1k(b *testing.B) {
	db, err := New()
	if err != nil {
		b.Fatal(err)
	}
	defer db.Close()
	seedWide(b, db, 1_000, 50)
	projection := ""
	for c := 0; c < 50; c++ {
		if c > 0 {
			projection += ", "
		}
		projection += fmt.Sprintf("n.p%d", c)
	}
	query := fmt.Sprintf("MATCH (n:Wide) RETURN %s", projection)
	b.ResetTimer()
	for i := 0; i < b.N; i++ {
		r, err := db.Execute(query, nil)
		if err != nil {
			b.Fatal(err)
		}
		if len(r.Rows) != 1_000 {
			b.Fatalf("expected 1000 rows, got %d", len(r.Rows))
		}
	}
}

func BenchmarkNestedList1k(b *testing.B) {
	db, err := New()
	if err != nil {
		b.Fatal(err)
	}
	defer db.Close()
	seedNested(b, db, 1_000, 10)
	b.ResetTimer()
	for i := 0; i < b.N; i++ {
		r, err := db.Execute("MATCH (n:Nested) RETURN n.tags", nil)
		if err != nil {
			b.Fatal(err)
		}
		if len(r.Rows) != 1_000 {
			b.Fatalf("expected 1000 rows, got %d", len(r.Rows))
		}
	}
}

func benchStream(b *testing.B, query string, want int) {
	db, err := New()
	if err != nil {
		b.Fatal(err)
	}
	defer db.Close()
	if _, err := db.Execute("UNWIND range(0, 9999) AS i CREATE (:Node {id: i, value: i % 100, name: 'node-' + toString(i)})", nil); err != nil {
		b.Fatal(err)
	}
	b.ResetTimer()
	for i := 0; i < b.N; i++ {
		it, err := db.Stream(query, nil)
		if err != nil {
			b.Fatal(err)
		}
		n := 0
		for it.Next() {
			_ = it.Row()
			n++
		}
		if err := it.Err(); err != nil {
			b.Fatal(err)
		}
		it.Close()
		if n != want {
			b.Fatalf("expected %d rows, got %d", want, n)
		}
	}
}

func BenchmarkStreamScan10k(b *testing.B) {
	benchStream(b, "MATCH (n:Node) RETURN n.id, n.value, n.name", 10_000)
}

func BenchmarkStreamNodes10k(b *testing.B) {
	benchStream(b, "MATCH (n:Node) RETURN n", 10_000)
}

func BenchmarkStreamWrite1k(b *testing.B) {
	db, err := New()
	if err != nil {
		b.Fatal(err)
	}
	defer db.Close()
	b.ResetTimer()
	for i := 0; i < b.N; i++ {
		it, err := db.Stream("UNWIND range(1, 1000) AS i CREATE (m:M {i: i}) RETURN m.i", nil)
		if err != nil {
			b.Fatal(err)
		}
		n := 0
		for it.Next() {
			n++
		}
		if err := it.Err(); err != nil {
			b.Fatal(err)
		}
		it.Close()
		if n != 1000 {
			b.Fatalf("expected 1000 rows, got %d", n)
		}
	}
}

func BenchmarkTransactionScan10k(b *testing.B) {
	db, err := New()
	if err != nil {
		b.Fatal(err)
	}
	defer db.Close()
	seedNodes(b, db, 10_000)
	stmts := []TransactionStatement{
		{Query: "MATCH (n:Node) RETURN n.id, n.value"},
		{Query: "MATCH (n:Node) WHERE n.id < 10 RETURN n"},
	}
	b.ResetTimer()
	for i := 0; i < b.N; i++ {
		rs, err := db.Transaction(stmts, TransactionReadOnly)
		if err != nil {
			b.Fatal(err)
		}
		if len(rs) != 2 || len(rs[0].Rows) != 10_000 || len(rs[1].Rows) != 10 {
			b.Fatalf("unexpected results")
		}
	}
}
