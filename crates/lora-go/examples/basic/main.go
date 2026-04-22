// Minimal end-to-end example: build a tiny graph, query it, print the
// rows. Matches the tone of `crates/lora-python/examples/basic.py`.
package main

import (
	"fmt"
	"log"

	lora "github.com/lora-db/lora/crates/lora-go"
)

func main() {
	db, err := lora.New()
	if err != nil {
		log.Fatalf("new: %v", err)
	}
	defer db.Close()

	if _, err := db.Execute(
		"CREATE (:Person {name: $n, born: $d})",
		lora.Params{"n": "Alice", "d": lora.Date("1990-01-15")},
	); err != nil {
		log.Fatalf("create: %v", err)
	}

	r, err := db.Execute(
		"MATCH (p:Person) RETURN p.name AS name, p.born AS born",
		nil,
	)
	if err != nil {
		log.Fatalf("match: %v", err)
	}

	fmt.Printf("columns: %v\n", r.Columns)
	for i, row := range r.Rows {
		fmt.Printf("row %d: %v\n", i, row)
	}

	nodes, _ := db.NodeCount()
	rels, _ := db.RelationshipCount()
	fmt.Printf("graph: nodes=%d relationships=%d version=%s\n",
		nodes, rels, lora.Version())
}
