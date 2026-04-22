package lora_test

import (
	"fmt"

	lora "github.com/lora-db/lora/crates/lora-go"
)

// ExampleDatabase shows the end-to-end shape of Create → Match →
// Return. It compiles as part of `go test ./...`.
func ExampleDatabase() {
	db, err := lora.New()
	if err != nil {
		fmt.Println("new:", err)
		return
	}
	defer db.Close()

	if _, err := db.Execute(
		"CREATE (:Person {name: $n})",
		lora.Params{"n": "Alice"},
	); err != nil {
		fmt.Println("create:", err)
		return
	}

	r, err := db.Execute("MATCH (p:Person) RETURN p.name AS name", nil)
	if err != nil {
		fmt.Println("match:", err)
		return
	}

	fmt.Println("columns:", r.Columns)
	fmt.Println("name:", r.Rows[0]["name"])
	// Output:
	// columns: [name]
	// name: Alice
}
