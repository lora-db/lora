# frozen_string_literal: true

# Basic sync usage example — mini social graph + typed query.

require "lora_ruby"

db = LoraRuby::Database.create # in-memory
# db = LoraRuby::Database.create("app", {"database_dir": "./data"}) # persistent: ./data/app.loradb

db.execute(
  "CREATE (:Person {name: 'Alice', age: 30}), " \
  "       (:Person {name: 'Bob', age: 28})",
)

db.execute(
  "MATCH (a:Person {name: $a}), (b:Person {name: $b}) " \
  "CREATE (a)-[:FOLLOWS]->(b)",
  { a: "Alice", b: "Bob" },
)

res = db.execute("MATCH (n:Person) RETURN n")
res["rows"].each do |row|
  n = row["n"]
  if LoraRuby.node?(n)
    puts "#{n['properties']['name']} (age: #{n['properties']['age']})"
  end
end
