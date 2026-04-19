#![allow(dead_code)]
//! Shared test infrastructure for the Lora integration test suite.
//!
//! Provides a `TestDb` wrapper around `Database<InMemoryGraph>` with helpers
//! for executing queries and extracting typed values from results.

use std::collections::BTreeMap;

use lora_database::{
    Database, ExecuteOptions, InMemoryGraph, LoraValue, QueryResult, ResultFormat,
};
use serde_json::Value as JsonValue;

/// A test harness wrapping a fresh in-memory graph and database.
pub struct TestDb {
    pub service: Database<InMemoryGraph>,
}

impl Default for TestDb {
    fn default() -> Self {
        Self::new()
    }
}

impl TestDb {
    /// Create a fresh, empty database.
    pub fn new() -> Self {
        Self {
            service: Database::in_memory(),
        }
    }

    /// Execute a Lora query and return the result in Rows format.
    pub fn exec(&self, cypher: &str) -> Result<QueryResult, anyhow::Error> {
        let options = Some(ExecuteOptions {
            format: ResultFormat::Rows,
        });
        self.service.execute(cypher, options)
    }

    /// Execute a Lora query with parameters and return the result in Rows format.
    pub fn exec_with_params(
        &self,
        cypher: &str,
        params: BTreeMap<String, LoraValue>,
    ) -> Result<QueryResult, anyhow::Error> {
        let options = Some(ExecuteOptions {
            format: ResultFormat::Rows,
        });
        self.service.execute_with_params(cypher, options, params)
    }

    /// Execute with parameters, panicking on error.
    pub fn run_with_params(
        &self,
        cypher: &str,
        params: BTreeMap<String, LoraValue>,
    ) -> Vec<JsonValue> {
        let result = self
            .exec_with_params(cypher, params)
            .unwrap_or_else(|e| panic!("query failed: {cypher}\nerror: {e}"));
        let json = serde_json::to_value(&result).unwrap();
        match json.get("rows") {
            Some(JsonValue::Array(rows)) => rows.clone(),
            _ => vec![],
        }
    }

    /// Execute and return rows as JSON values for easy assertion.
    pub fn exec_json(&self, cypher: &str) -> Result<Vec<JsonValue>, anyhow::Error> {
        let result = self.exec(cypher)?;
        let json = serde_json::to_value(&result)?;
        match json.get("rows") {
            Some(JsonValue::Array(rows)) => Ok(rows.clone()),
            _ => Ok(vec![]),
        }
    }

    /// Execute and return the number of result rows.
    pub fn exec_count(&self, cypher: &str) -> Result<usize, anyhow::Error> {
        Ok(self.exec_json(cypher)?.len())
    }

    /// Execute expecting success, panicking on error.
    pub fn run(&self, cypher: &str) -> Vec<JsonValue> {
        self.exec_json(cypher)
            .unwrap_or_else(|e| panic!("query failed: {cypher}\nerror: {e}"))
    }

    /// Execute expecting an error, panicking on success.
    pub fn run_err(&self, cypher: &str) -> String {
        match self.exec(cypher) {
            Err(e) => e.to_string(),
            Ok(_) => panic!("expected error for query: {cypher}"),
        }
    }

    /// Execute and assert an exact row count.
    pub fn assert_count(&self, cypher: &str, expected: usize) {
        let rows = self.run(cypher);
        assert_eq!(
            rows.len(),
            expected,
            "expected {expected} rows for: {cypher}\ngot: {rows:?}"
        );
    }

    /// Execute and return a single scalar from the first row, first column.
    pub fn scalar(&self, cypher: &str) -> JsonValue {
        let rows = self.run(cypher);
        assert!(
            !rows.is_empty(),
            "expected at least one row for scalar: {cypher}"
        );
        let row = &rows[0];
        match row {
            JsonValue::Object(map) => map.values().next().cloned().unwrap_or(JsonValue::Null),
            _ => JsonValue::Null,
        }
    }

    /// Execute and return all values for a given column name.
    pub fn column(&self, cypher: &str, col: &str) -> Vec<JsonValue> {
        self.run(cypher)
            .iter()
            .map(|row| row.get(col).cloned().unwrap_or(JsonValue::Null))
            .collect()
    }

    /// Seed a standard social graph for relationship tests:
    /// Alice -[:FOLLOWS]-> Bob -[:FOLLOWS]-> Carol
    /// Alice -[:KNOWS]-> Carol
    pub fn seed_social_graph(&self) {
        self.run("CREATE (a:User {name: 'Alice', age: 30})");
        self.run("CREATE (b:User {name: 'Bob', age: 25})");
        self.run("CREATE (c:User {name: 'Carol', age: 35})");
        self.run(
            "MATCH (a:User {name: 'Alice'}), (b:User {name: 'Bob'}) \
             CREATE (a)-[:FOLLOWS {since: 2020}]->(b)",
        );
        self.run(
            "MATCH (b:User {name: 'Bob'}), (c:User {name: 'Carol'}) \
             CREATE (b)-[:FOLLOWS {since: 2021}]->(c)",
        );
        self.run(
            "MATCH (a:User {name: 'Alice'}), (c:User {name: 'Carol'}) \
             CREATE (a)-[:KNOWS]->(c)",
        );
    }

    /// Seed a company org-chart graph with multiple node types and relationship types.
    ///
    /// Nodes (12):
    ///   (:Person {name:'Alice', age:35, dept:'Engineering'})
    ///   (:Person {name:'Bob',   age:28, dept:'Engineering'})
    ///   (:Person {name:'Carol', age:42, dept:'Marketing'})
    ///   (:Person {name:'Dave',  age:31, dept:'Marketing'})
    ///   (:Person {name:'Eve',   age:26, dept:'Engineering'})
    ///   (:Person:Manager {name:'Frank', age:50, dept:'Engineering'})
    ///   (:Company {name:'Acme', founded: 2010})
    ///   (:Project {name:'Alpha', budget: 100000})
    ///   (:Project {name:'Beta',  budget: 50000})
    ///   (:City {name:'London'})
    ///   (:City {name:'Berlin'})
    ///   (:City {name:'Tokyo'})
    ///
    /// Relationships:
    ///   Alice  -[:WORKS_AT {since:2018}]-> Acme
    ///   Bob    -[:WORKS_AT {since:2020}]-> Acme
    ///   Carol  -[:WORKS_AT {since:2015}]-> Acme
    ///   Dave   -[:WORKS_AT {since:2021}]-> Acme
    ///   Eve    -[:WORKS_AT {since:2022}]-> Acme
    ///   Frank  -[:WORKS_AT {since:2012}]-> Acme
    ///   Frank  -[:MANAGES]-> Alice
    ///   Frank  -[:MANAGES]-> Bob
    ///   Frank  -[:MANAGES]-> Eve
    ///   Carol  -[:MANAGES]-> Dave
    ///   Alice  -[:ASSIGNED_TO {role:'lead'}]-> Alpha
    ///   Bob    -[:ASSIGNED_TO {role:'dev'}]-> Alpha
    ///   Carol  -[:ASSIGNED_TO {role:'lead'}]-> Beta
    ///   Eve    -[:ASSIGNED_TO {role:'dev'}]-> Beta
    ///   Alice  -[:LIVES_IN]-> London
    ///   Bob    -[:LIVES_IN]-> Berlin
    ///   Carol  -[:LIVES_IN]-> London
    ///   Dave   -[:LIVES_IN]-> Tokyo
    ///   Eve    -[:LIVES_IN]-> Berlin
    ///   Frank  -[:LIVES_IN]-> London
    pub fn seed_org_graph(&self) {
        // Nodes
        self.run("CREATE (:Person {name:'Alice', age:35, dept:'Engineering'})");
        self.run("CREATE (:Person {name:'Bob',   age:28, dept:'Engineering'})");
        self.run("CREATE (:Person {name:'Carol', age:42, dept:'Marketing'})");
        self.run("CREATE (:Person {name:'Dave',  age:31, dept:'Marketing'})");
        self.run("CREATE (:Person {name:'Eve',   age:26, dept:'Engineering'})");
        self.run("CREATE (:Person:Manager {name:'Frank', age:50, dept:'Engineering'})");
        self.run("CREATE (:Company {name:'Acme', founded: 2010})");
        self.run("CREATE (:Project {name:'Alpha', budget: 100000})");
        self.run("CREATE (:Project {name:'Beta',  budget: 50000})");
        self.run("CREATE (:City {name:'London'})");
        self.run("CREATE (:City {name:'Berlin'})");
        self.run("CREATE (:City {name:'Tokyo'})");

        // WORKS_AT
        for (person, since) in [
            ("Alice", 2018),
            ("Bob", 2020),
            ("Carol", 2015),
            ("Dave", 2021),
            ("Eve", 2022),
            ("Frank", 2012),
        ] {
            self.run(&format!(
                "MATCH (p:Person {{name:'{person}'}}), (c:Company {{name:'Acme'}}) \
                 CREATE (p)-[:WORKS_AT {{since:{since}}}]->(c)"
            ));
        }

        // MANAGES
        for (mgr, sub) in [
            ("Frank", "Alice"),
            ("Frank", "Bob"),
            ("Frank", "Eve"),
            ("Carol", "Dave"),
        ] {
            self.run(&format!(
                "MATCH (m:Person {{name:'{mgr}'}}), (s:Person {{name:'{sub}'}}) \
                 CREATE (m)-[:MANAGES]->(s)"
            ));
        }

        // ASSIGNED_TO
        for (person, project, role) in [
            ("Alice", "Alpha", "lead"),
            ("Bob", "Alpha", "dev"),
            ("Carol", "Beta", "lead"),
            ("Eve", "Beta", "dev"),
        ] {
            self.run(&format!(
                "MATCH (p:Person {{name:'{person}'}}), (pr:Project {{name:'{project}'}}) \
                 CREATE (p)-[:ASSIGNED_TO {{role:'{role}'}}]->(pr)"
            ));
        }

        // LIVES_IN
        for (person, city) in [
            ("Alice", "London"),
            ("Bob", "Berlin"),
            ("Carol", "London"),
            ("Dave", "Tokyo"),
            ("Eve", "Berlin"),
            ("Frank", "London"),
        ] {
            self.run(&format!(
                "MATCH (p:Person {{name:'{person}'}}), (c:City {{name:'{city}'}}) \
                 CREATE (p)-[:LIVES_IN]->(c)"
            ));
        }
    }

    /// Seed a linear chain: n0->n1->n2->...->n(len-1) with [:NEXT] relationships.
    pub fn seed_chain(&self, len: usize) {
        for i in 0..len {
            self.run(&format!("CREATE (:Chain {{idx:{i}}})"));
        }
        for i in 0..len.saturating_sub(1) {
            self.run(&format!(
                "MATCH (a:Chain {{idx:{i}}}), (b:Chain {{idx:{next}}}) CREATE (a)-[:NEXT]->(b)",
                next = i + 1
            ));
        }
    }

    /// Seed a cycle: n0->n1->n2->...->n(len-1)->n0 with [:LOOP] relationships.
    pub fn seed_cycle(&self, len: usize) {
        for i in 0..len {
            self.run(&format!("CREATE (:Ring {{idx:{i}}})"));
        }
        for i in 0..len {
            let next = (i + 1) % len;
            self.run(&format!(
                "MATCH (a:Ring {{idx:{i}}}), (b:Ring {{idx:{next}}}) CREATE (a)-[:LOOP]->(b)"
            ));
        }
    }

    /// Extract a sorted list of string values from a column.
    pub fn sorted_strings(&self, cypher: &str, col: &str) -> Vec<String> {
        let mut v: Vec<String> = self
            .column(cypher, col)
            .iter()
            .filter_map(|j| j.as_str().map(String::from))
            .collect();
        v.sort();
        v
    }

    /// Extract a sorted list of i64 values from a column.
    pub fn sorted_ints(&self, cypher: &str, col: &str) -> Vec<i64> {
        let mut v: Vec<i64> = self
            .column(cypher, col)
            .iter()
            .filter_map(|j| j.as_i64())
            .collect();
        v.sort();
        v
    }

    /// Seed a package dependency graph for transitive-dependency tests.
    ///
    /// Nodes (6):
    ///   (:Package {name:'app',    version:'1.0'})
    ///   (:Package {name:'web',    version:'2.1'})
    ///   (:Package {name:'auth',   version:'0.9'})
    ///   (:Package {name:'crypto', version:'1.2'})
    ///   (:Package {name:'log',    version:'3.0'})
    ///   (:Package {name:'util',   version:'1.0'})
    ///
    /// Relationships:
    ///   app  -[:DEPENDS_ON]-> web
    ///   app  -[:DEPENDS_ON]-> auth
    ///   app  -[:DEPENDS_ON]-> log
    ///   web  -[:DEPENDS_ON]-> log
    ///   web  -[:DEPENDS_ON]-> util
    ///   auth -[:DEPENDS_ON]-> crypto
    ///   auth -[:DEPENDS_ON]-> log
    ///   crypto -[:DEPENDS_ON]-> util
    pub fn seed_dependency_graph(&self) {
        for (name, ver) in [
            ("app", "1.0"),
            ("web", "2.1"),
            ("auth", "0.9"),
            ("crypto", "1.2"),
            ("log", "3.0"),
            ("util", "1.0"),
        ] {
            self.run(&format!(
                "CREATE (:Package {{name:'{name}', version:'{ver}'}})"
            ));
        }
        for (from, to) in [
            ("app", "web"),
            ("app", "auth"),
            ("app", "log"),
            ("web", "log"),
            ("web", "util"),
            ("auth", "crypto"),
            ("auth", "log"),
            ("crypto", "util"),
        ] {
            self.run(&format!(
                "MATCH (a:Package {{name:'{from}'}}), (b:Package {{name:'{to}'}}) CREATE (a)-[:DEPENDS_ON]->(b)"
            ));
        }
    }

    /// Seed a transport/route graph for network-style queries.
    ///
    /// Nodes (5):
    ///   (:Station {name:'Amsterdam', zone:1})
    ///   (:Station {name:'Rotterdam', zone:2})
    ///   (:Station {name:'Utrecht',   zone:1})
    ///   (:Station {name:'Den Haag',  zone:2})
    ///   (:Station {name:'Eindhoven', zone:3})
    ///
    /// Relationships (bidirectional routes):
    ///   Amsterdam <-[:ROUTE {distance:40,  duration:25}]-> Utrecht
    ///   Amsterdam <-[:ROUTE {distance:60,  duration:40}]-> Rotterdam
    ///   Utrecht   <-[:ROUTE {distance:55,  duration:35}]-> Rotterdam
    ///   Rotterdam <-[:ROUTE {distance:25,  duration:15}]-> Den Haag
    ///   Utrecht   <-[:ROUTE {distance:100, duration:60}]-> Eindhoven
    pub fn seed_transport_graph(&self) {
        for (name, zone) in [
            ("Amsterdam", 1),
            ("Rotterdam", 2),
            ("Utrecht", 1),
            ("Den Haag", 2),
            ("Eindhoven", 3),
        ] {
            self.run(&format!("CREATE (:Station {{name:'{name}', zone:{zone}}})"));
        }
        for (a, b, dist, dur) in [
            ("Amsterdam", "Utrecht", 40, 25),
            ("Amsterdam", "Rotterdam", 60, 40),
            ("Utrecht", "Rotterdam", 55, 35),
            ("Rotterdam", "Den Haag", 25, 15),
            ("Utrecht", "Eindhoven", 100, 60),
        ] {
            self.run(&format!(
                "MATCH (s1:Station {{name:'{a}'}}), (s2:Station {{name:'{b}'}}) \
                 CREATE (s1)-[:ROUTE {{distance:{dist}, duration:{dur}}}]->(s2)"
            ));
            self.run(&format!(
                "MATCH (s1:Station {{name:'{b}'}}), (s2:Station {{name:'{a}'}}) \
                 CREATE (s1)-[:ROUTE {{distance:{dist}, duration:{dur}}}]->(s2)"
            ));
        }
    }

    /// Seed a recommendation graph for user-item-rating patterns.
    ///
    /// Nodes:
    ///   (:Viewer {name:'Alice'}), (:Viewer {name:'Bob'}), (:Viewer {name:'Carol'})
    ///   (:Movie {title:'Matrix', year:1999, genre:'sci-fi'})
    ///   (:Movie {title:'Inception', year:2010, genre:'sci-fi'})
    ///   (:Movie {title:'Amelie', year:2001, genre:'drama'})
    ///   (:Movie {title:'Jaws', year:1975, genre:'thriller'})
    ///
    /// Relationships:
    ///   Alice -[:RATED {score:5}]-> Matrix
    ///   Alice -[:RATED {score:4}]-> Inception
    ///   Alice -[:RATED {score:3}]-> Amelie
    ///   Bob   -[:RATED {score:5}]-> Matrix
    ///   Bob   -[:RATED {score:2}]-> Jaws
    ///   Carol -[:RATED {score:4}]-> Amelie
    ///   Carol -[:RATED {score:5}]-> Inception
    /// Seed a rich social graph for advanced relationship tests.
    ///
    /// Nodes (10):
    ///   (:Person {name:'Alice', age:30, city:'London'})
    ///   (:Person {name:'Bob', age:25, city:'Berlin'})
    ///   (:Person {name:'Carol', age:35, city:'London'})
    ///   (:Person {name:'Dave', age:28, city:'Paris'})
    ///   (:Person:Influencer {name:'Eve', age:32, city:'Berlin'})
    ///   (:Person {name:'Frank', age:40, city:'London'})
    ///   (:Interest {name:'Music'})
    ///   (:Interest {name:'Sports'})
    ///   (:Interest {name:'Travel'})
    ///   (:Interest {name:'Cooking'})
    ///
    /// Relationships:
    ///   Alice  -[:KNOWS {since:2015, strength:5}]-> Bob
    ///   Alice  -[:KNOWS {since:2018, strength:8}]-> Carol
    ///   Bob    -[:KNOWS {since:2019, strength:4}]-> Carol
    ///   Bob    -[:KNOWS {since:2020, strength:3}]-> Dave
    ///   Carol  -[:KNOWS {since:2017, strength:6}]-> Eve
    ///   Dave   -[:KNOWS {since:2021, strength:2}]-> Eve
    ///   Eve    -[:KNOWS {since:2016, strength:7}]-> Frank
    ///   Alice  -[:FOLLOWS]-> Carol
    ///   Alice  -[:FOLLOWS]-> Eve
    ///   Bob    -[:FOLLOWS]-> Alice
    ///   Carol  -[:FOLLOWS]-> Frank
    ///   Dave   -[:FOLLOWS]-> Alice
    ///   Frank  -[:FOLLOWS]-> Bob
    ///   Alice  -[:BLOCKED]-> Frank
    ///   Dave   -[:BLOCKED]-> Carol
    ///   Alice  -[:INTERESTED_IN {level:'high'}]->   Music
    ///   Alice  -[:INTERESTED_IN {level:'medium'}]-> Travel
    ///   Bob    -[:INTERESTED_IN {level:'high'}]->   Sports
    ///   Bob    -[:INTERESTED_IN {level:'low'}]->    Music
    ///   Carol  -[:INTERESTED_IN {level:'high'}]->   Cooking
    ///   Carol  -[:INTERESTED_IN {level:'medium'}]-> Travel
    ///   Dave   -[:INTERESTED_IN {level:'high'}]->   Sports
    ///   Dave   -[:INTERESTED_IN {level:'high'}]->   Music
    ///   Eve    -[:INTERESTED_IN {level:'medium'}]-> Music
    ///   Eve    -[:INTERESTED_IN {level:'high'}]->   Travel
    ///   Frank  -[:INTERESTED_IN {level:'low'}]->    Cooking
    pub fn seed_rich_social_graph(&self) {
        // People
        for (name, age, city) in [
            ("Alice", 30, "London"),
            ("Bob", 25, "Berlin"),
            ("Carol", 35, "London"),
            ("Dave", 28, "Paris"),
            ("Frank", 40, "London"),
        ] {
            self.run(&format!(
                "CREATE (:Person {{name:'{name}', age:{age}, city:'{city}'}})"
            ));
        }
        // Eve has an extra label
        self.run("CREATE (:Person:Influencer {name:'Eve', age:32, city:'Berlin'})");

        // Interests
        for interest in ["Music", "Sports", "Travel", "Cooking"] {
            self.run(&format!("CREATE (:Interest {{name:'{interest}'}})"));
        }

        // KNOWS (bidirectional-style friendships, stored as directed edges)
        for (a, b, since, strength) in [
            ("Alice", "Bob", 2015, 5),
            ("Alice", "Carol", 2018, 8),
            ("Bob", "Carol", 2019, 4),
            ("Bob", "Dave", 2020, 3),
            ("Carol", "Eve", 2017, 6),
            ("Dave", "Eve", 2021, 2),
            ("Eve", "Frank", 2016, 7),
        ] {
            self.run(&format!(
                "MATCH (a:Person {{name:'{a}'}}), (b:Person {{name:'{b}'}}) \
                 CREATE (a)-[:KNOWS {{since:{since}, strength:{strength}}}]->(b)"
            ));
        }

        // FOLLOWS
        for (follower, followed) in [
            ("Alice", "Carol"),
            ("Alice", "Eve"),
            ("Bob", "Alice"),
            ("Carol", "Frank"),
            ("Dave", "Alice"),
            ("Frank", "Bob"),
        ] {
            self.run(&format!(
                "MATCH (a:Person {{name:'{follower}'}}), (b:Person {{name:'{followed}'}}) \
                 CREATE (a)-[:FOLLOWS]->(b)"
            ));
        }

        // BLOCKED
        for (blocker, blocked) in [("Alice", "Frank"), ("Dave", "Carol")] {
            self.run(&format!(
                "MATCH (a:Person {{name:'{blocker}'}}), (b:Person {{name:'{blocked}'}}) \
                 CREATE (a)-[:BLOCKED]->(b)"
            ));
        }

        // INTERESTED_IN
        for (person, interest, level) in [
            ("Alice", "Music", "high"),
            ("Alice", "Travel", "medium"),
            ("Bob", "Sports", "high"),
            ("Bob", "Music", "low"),
            ("Carol", "Cooking", "high"),
            ("Carol", "Travel", "medium"),
            ("Dave", "Sports", "high"),
            ("Dave", "Music", "high"),
            ("Eve", "Music", "medium"),
            ("Eve", "Travel", "high"),
            ("Frank", "Cooking", "low"),
        ] {
            self.run(&format!(
                "MATCH (p:Person {{name:'{person}'}}), (i:Interest {{name:'{interest}'}}) \
                 CREATE (p)-[:INTERESTED_IN {{level:'{level}'}}]->(i)"
            ));
        }
    }

    /// Seed a knowledge graph for entity linking and dense traversal tests.
    ///
    /// Nodes (14):
    ///   (:Entity {name:'Albert Einstein', type:'person'})
    ///   (:Entity {name:'Physics', type:'field'})
    ///   (:Entity {name:'Mathematics', type:'field'})
    ///   (:Entity {name:'General Relativity', type:'theory'})
    ///   (:Entity {name:'Quantum Mechanics', type:'theory'})
    ///   (:Entity {name:'Nobel Prize', type:'award'})
    ///   (:Document {title:'On the Electrodynamics of Moving Bodies', year:1905})
    ///   (:Document {title:'The Foundation of General Relativity', year:1916})
    ///   (:Topic {name:'Theoretical Physics'})
    ///   (:Topic {name:'Cosmology'})
    ///   (:Alias {value:'Einstein'})
    ///   (:Alias {value:'A. Einstein'})
    ///   (:Entity {name:'Marie Curie', type:'person'})
    ///   (:Entity {name:'Radioactivity', type:'field'})
    ///
    /// Rich relationship network for dense traversal and entity resolution.
    pub fn seed_knowledge_graph(&self) {
        // Entities
        for (name, etype) in [
            ("Albert Einstein", "person"),
            ("Physics", "field"),
            ("Mathematics", "field"),
            ("General Relativity", "theory"),
            ("Quantum Mechanics", "theory"),
            ("Nobel Prize", "award"),
            ("Marie Curie", "person"),
            ("Radioactivity", "field"),
        ] {
            self.run(&format!(
                "CREATE (:Entity {{name:'{name}', type:'{etype}'}})"
            ));
        }

        // Documents
        self.run("CREATE (:Document {title:'On the Electrodynamics of Moving Bodies', year:1905})");
        self.run("CREATE (:Document {title:'The Foundation of General Relativity', year:1916})");

        // Topics
        for topic in ["Theoretical Physics", "Cosmology"] {
            self.run(&format!("CREATE (:Topic {{name:'{topic}'}})"));
        }

        // Aliases
        for alias in ["Einstein", "A. Einstein"] {
            self.run(&format!("CREATE (:Alias {{value:'{alias}'}})"));
        }

        // Einstein relationships
        let einstein = "Albert Einstein";
        for field in ["Physics", "Mathematics"] {
            self.run(&format!(
                "MATCH (e:Entity {{name:'{einstein}'}}), (f:Entity {{name:'{field}'}}) \
                 CREATE (e)-[:STUDIED]->(f)"
            ));
        }
        self.run(&format!(
            "MATCH (e:Entity {{name:'{einstein}'}}), (t:Entity {{name:'General Relativity'}}) \
             CREATE (e)-[:PROPOSED]->(t)"
        ));
        self.run(&format!(
            "MATCH (e:Entity {{name:'{einstein}'}}), (t:Entity {{name:'Quantum Mechanics'}}) \
             CREATE (e)-[:CONTRIBUTED_TO]->(t)"
        ));
        self.run(&format!(
            "MATCH (e:Entity {{name:'{einstein}'}}), (a:Entity {{name:'Nobel Prize'}}) \
             CREATE (e)-[:RECEIVED {{year:1921}}]->(a)"
        ));

        // Authored
        self.run(&format!(
            "MATCH (e:Entity {{name:'{einstein}'}}), \
                   (d:Document {{title:'On the Electrodynamics of Moving Bodies'}}) \
             CREATE (e)-[:AUTHORED]->(d)"
        ));
        self.run(&format!(
            "MATCH (e:Entity {{name:'{einstein}'}}), \
                   (d:Document {{title:'The Foundation of General Relativity'}}) \
             CREATE (e)-[:AUTHORED]->(d)"
        ));

        // Document -> theory
        for doc_year in [1905, 1916] {
            self.run(&format!(
                "MATCH (d:Document {{year:{doc_year}}}), (t:Entity {{name:'General Relativity'}}) \
                 CREATE (d)-[:ABOUT]->(t)"
            ));
        }

        // Theory -> Topic
        for theory in ["General Relativity", "Quantum Mechanics"] {
            self.run(&format!(
                "MATCH (t:Entity {{name:'{theory}'}}), (tp:Topic {{name:'Theoretical Physics'}}) \
                 CREATE (t)-[:BELONGS_TO]->(tp)"
            ));
        }
        self.run(
            "MATCH (t:Entity {name:'General Relativity'}), (tp:Topic {name:'Cosmology'}) \
             CREATE (t)-[:RELATES_TO]->(tp)",
        );

        // Aliases
        for alias in ["Einstein", "A. Einstein"] {
            self.run(&format!(
                "MATCH (e:Entity {{name:'{einstein}'}}), (a:Alias {{value:'{alias}'}}) \
                 CREATE (e)-[:HAS_ALIAS]->(a)"
            ));
        }

        // Field hierarchy
        self.run(
            "MATCH (f:Entity {name:'Physics'}), (tp:Topic {name:'Theoretical Physics'}) \
             CREATE (f)-[:PARENT_OF]->(tp)",
        );

        // Marie Curie relationships
        self.run(
            "MATCH (mc:Entity {name:'Marie Curie'}), (r:Entity {name:'Radioactivity'}) \
             CREATE (mc)-[:STUDIED]->(r)",
        );
        self.run(
            "MATCH (mc:Entity {name:'Marie Curie'}), (p:Entity {name:'Physics'}) \
             CREATE (mc)-[:STUDIED]->(p)",
        );
        self.run(
            "MATCH (mc:Entity {name:'Marie Curie'}), (np:Entity {name:'Nobel Prize'}) \
             CREATE (mc)-[:RECEIVED {year:1903}]->(np)",
        );
        // Both scientists received the Nobel Prize — shared node
        // Curie also contributed to Quantum Mechanics
        self.run(
            "MATCH (mc:Entity {name:'Marie Curie'}), (qm:Entity {name:'Quantum Mechanics'}) \
             CREATE (mc)-[:CONTRIBUTED_TO]->(qm)",
        );
    }

    pub fn seed_recommendation_graph(&self) {
        for name in ["Alice", "Bob", "Carol"] {
            self.run(&format!("CREATE (:Viewer {{name:'{name}'}})"));
        }
        for (title, year, genre) in [
            ("Matrix", 1999, "sci-fi"),
            ("Inception", 2010, "sci-fi"),
            ("Amelie", 2001, "drama"),
            ("Jaws", 1975, "thriller"),
        ] {
            self.run(&format!(
                "CREATE (:Movie {{title:'{title}', year:{year}, genre:'{genre}'}})"
            ));
        }
        for (viewer, movie, score) in [
            ("Alice", "Matrix", 5),
            ("Alice", "Inception", 4),
            ("Alice", "Amelie", 3),
            ("Bob", "Matrix", 5),
            ("Bob", "Jaws", 2),
            ("Carol", "Amelie", 4),
            ("Carol", "Inception", 5),
        ] {
            self.run(&format!(
                "MATCH (v:Viewer {{name:'{viewer}'}}), (m:Movie {{title:'{movie}'}}) \
                 CREATE (v)-[:RATED {{score:{score}}}]->(m)"
            ));
        }
    }
}
