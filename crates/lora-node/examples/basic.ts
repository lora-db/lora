/**
 * Basic usage example — create a small social graph and run typed queries.
 *
 * Run with:
 *   npm run build
 *   node --loader tsx examples/basic.ts
 *
 * or compile with tsc and run the JS output.
 */

import { Database, isNode, type LoraNode } from "../ts/index.js";

async function main() {
  const db = await Database.create();

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
