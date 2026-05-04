/**
 * Node.js example — uses the WASM engine in-process via the main-thread
 * Database class. Suitable for scripts, tests, and small server-side usage.
 */

import { createDatabase, isNode, type LoraNode } from "../ts/index.js";

async function main(): Promise<void> {
  const db = await createDatabase();

  await db.execute(
    "CREATE (:Person {name: 'Alice', age: 30}), (:Person {name: 'Bob', age: 28})",
  );
  await db.execute(
    "MATCH (a:Person {name: $a}), (b:Person {name: $b}) CREATE (a)-[:FOLLOWS]->(b)",
    { a: "Alice", b: "Bob" },
  );

  const result = await db.execute<{ n: LoraNode }>("MATCH (n:Person) RETURN n");
  for (const row of result.rows) {
    if (isNode(row.n)) {
      console.log(`${row.n.properties.name} (age: ${row.n.properties.age})`);
    }
  }
}

main().catch((err) => {
  console.error(err);
  process.exit(1);
});
