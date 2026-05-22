//! Translate parsed rows into Cypher `CREATE` (or `MATCH … CREATE`) statements.
//!
//! Two flavours:
//!   * [`RowMapping::Node`] turns each row into one node, optionally
//!     keyed by an `id` column so a follow-up [`RowMapping::Relationship`]
//!     can match start/end nodes by that key.
//!   * [`RowMapping::Relationship`] looks up two existing nodes and
//!     wires a typed relationship between them.
//!
//! The generated statement uses `UNWIND $rows AS r CREATE …` so the
//! import driver can stream batches through a single parameterized
//! transaction call instead of compiling a new query per row.

use serde::{Deserialize, Serialize};

/// One file column → one target property. `source` is the column
/// name as it appears in the file (after schema-marker
/// canonicalisation, e.g. `_id` for `:ID`); `property` is the target
/// node/relationship property name. When equal, the mapping is the
/// identity — useful default for the auto-mapping path.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ColumnSpec {
    pub source: String,
    pub property: String,
}

impl ColumnSpec {
    pub fn identity(name: impl Into<String>) -> Self {
        let n = name.into();
        Self {
            source: n.clone(),
            property: n,
        }
    }
}

/// Top-level mapping: how to turn a stream of decoded rows into
/// Cypher inserts. Carries enough information for the playground's
/// auto-detect / preview path; users can edit this in the import
/// dialog before running.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum RowMapping {
    /// Each row becomes a node of `label` with the listed properties.
    Node {
        /// Single label or `Label1:Label2:Label3`.
        label: String,
        /// Optional file column whose value becomes a unique property
        /// later referenced by relationship imports. When set, the
        /// generated statement includes a property literal; when
        /// omitted, the node has no identity beyond Cypher's
        /// auto-allocated node id.
        id_column: Option<String>,
        /// Property name to store the id column under (defaults to
        /// `id` when [`Self::id_column`] is set).
        id_property: Option<String>,
        /// Source column → property name mappings.
        properties: Vec<ColumnSpec>,
    },
    /// Each row becomes a relationship between two nodes located by
    /// `(start_label, start_match_property)` and the corresponding
    /// `end_*` fields.
    Relationship {
        rel_type: String,
        start_label: String,
        start_column: String,
        start_match_property: String,
        end_label: String,
        end_column: String,
        end_match_property: String,
        properties: Vec<ColumnSpec>,
    },
}

/// Quote an identifier (label, property, parameter) safely. The
/// engine's Cypher dialect supports backticks for identifiers
/// containing reserved chars, so we delegate to that — and reject
/// identifiers that contain a backtick themselves since we can't
/// escape them without ambiguity.
fn quote_ident(name: &str) -> Result<String, String> {
    if name.is_empty() {
        return Err("identifier must not be empty".into());
    }
    if name.contains('`') {
        return Err(format!("identifier `{name}` contains a backtick"));
    }
    let safe_simple = name.chars().enumerate().all(|(i, c)| {
        if i == 0 {
            c.is_ascii_alphabetic() || c == '_'
        } else {
            c.is_ascii_alphanumeric() || c == '_'
        }
    });
    if safe_simple {
        Ok(name.to_string())
    } else {
        Ok(format!("`{name}`"))
    }
}

fn quote_param(name: &str) -> Result<String, String> {
    if name.is_empty() {
        return Err("parameter name must not be empty".into());
    }
    if name.contains('`') {
        return Err(format!("parameter `{name}` contains a backtick"));
    }
    Ok(format!("r.{}", quote_ident(name)?))
}

/// Build the Cypher template for a node mapping. The result expects
/// a `$rows` parameter containing an array of objects whose keys
/// match the source column names referenced by the mapping.
pub fn parameterized_create_for_node(
    label: &str,
    id_column: Option<&str>,
    id_property: Option<&str>,
    properties: &[ColumnSpec],
) -> Result<String, String> {
    let label_part = label
        .split(':')
        .filter(|s| !s.is_empty())
        .map(quote_ident)
        .collect::<Result<Vec<_>, _>>()?
        .join(":");
    if label_part.is_empty() {
        return Err("node mapping requires at least one label".into());
    }

    let mut prop_pairs: Vec<String> = Vec::new();
    if let Some(col) = id_column {
        let prop = id_property.unwrap_or("id");
        prop_pairs.push(format!("{}: {}", quote_ident(prop)?, quote_param(col)?));
    }
    for spec in properties {
        if Some(spec.source.as_str()) == id_column {
            continue; // already wired through the id column
        }
        prop_pairs.push(format!(
            "{}: {}",
            quote_ident(&spec.property)?,
            quote_param(&spec.source)?
        ));
    }

    let props_block = if prop_pairs.is_empty() {
        String::new()
    } else {
        format!(" {{{}}}", prop_pairs.join(", "))
    };
    Ok(format!(
        "UNWIND $rows AS r CREATE (:{label_part}{props_block})"
    ))
}

/// Build the Cypher template for a relationship mapping.
#[allow(clippy::too_many_arguments)]
pub fn parameterized_create_for_relationship(
    rel_type: &str,
    start_label: &str,
    start_column: &str,
    start_match_property: &str,
    end_label: &str,
    end_column: &str,
    end_match_property: &str,
    properties: &[ColumnSpec],
) -> Result<String, String> {
    let start_label = quote_ident(start_label)?;
    let end_label = quote_ident(end_label)?;
    let rel_type = quote_ident(rel_type)?;
    let start_match_property = quote_ident(start_match_property)?;
    let end_match_property = quote_ident(end_match_property)?;
    let start_param = quote_param(start_column)?;
    let end_param = quote_param(end_column)?;

    let mut prop_pairs: Vec<String> = Vec::new();
    for spec in properties {
        if spec.source == start_column || spec.source == end_column {
            continue;
        }
        prop_pairs.push(format!(
            "{}: {}",
            quote_ident(&spec.property)?,
            quote_param(&spec.source)?
        ));
    }
    let props_block = if prop_pairs.is_empty() {
        String::new()
    } else {
        format!(" {{{}}}", prop_pairs.join(", "))
    };

    Ok(format!(
        "UNWIND $rows AS r \
         MATCH (a:{start_label} {{{start_match_property}: {start_param}}}), \
         (b:{end_label} {{{end_match_property}: {end_param}}}) \
         CREATE (a)-[:{rel_type}{props_block}]->(b)"
    ))
}

impl RowMapping {
    /// Render the mapping into a Cypher template. The template binds
    /// `$rows` to a batch of decoded rows.
    pub fn to_cypher(&self) -> Result<String, String> {
        match self {
            RowMapping::Node {
                label,
                id_column,
                id_property,
                properties,
            } => parameterized_create_for_node(
                label,
                id_column.as_deref(),
                id_property.as_deref(),
                properties,
            ),
            RowMapping::Relationship {
                rel_type,
                start_label,
                start_column,
                start_match_property,
                end_label,
                end_column,
                end_match_property,
                properties,
            } => parameterized_create_for_relationship(
                rel_type,
                start_label,
                start_column,
                start_match_property,
                end_label,
                end_column,
                end_match_property,
                properties,
            ),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn node_with_id_and_props() {
        let m = RowMapping::Node {
            label: "User".into(),
            id_column: Some("uid".into()),
            id_property: Some("uid".into()),
            properties: vec![ColumnSpec::identity("name"), ColumnSpec::identity("age")],
        };
        let cypher = m.to_cypher().unwrap();
        assert_eq!(
            cypher,
            "UNWIND $rows AS r CREATE (:User {uid: r.uid, name: r.name, age: r.age})"
        );
    }

    #[test]
    fn node_without_id() {
        let m = RowMapping::Node {
            label: "Tag".into(),
            id_column: None,
            id_property: None,
            properties: vec![ColumnSpec::identity("name")],
        };
        let cypher = m.to_cypher().unwrap();
        assert_eq!(cypher, "UNWIND $rows AS r CREATE (:Tag {name: r.name})");
    }

    #[test]
    fn multi_label_node() {
        let m = RowMapping::Node {
            label: "User:Admin".into(),
            id_column: None,
            id_property: None,
            properties: vec![],
        };
        let cypher = m.to_cypher().unwrap();
        assert_eq!(cypher, "UNWIND $rows AS r CREATE (:User:Admin)");
    }

    #[test]
    fn relationship_basic() {
        let m = RowMapping::Relationship {
            rel_type: "FOLLOWS".into(),
            start_label: "User".into(),
            start_column: "src".into(),
            start_match_property: "uid".into(),
            end_label: "User".into(),
            end_column: "dst".into(),
            end_match_property: "uid".into(),
            properties: vec![ColumnSpec::identity("since")],
        };
        let cypher = m.to_cypher().unwrap();
        assert!(cypher.contains("MATCH (a:User {uid: r.src})"));
        assert!(cypher.contains("(b:User {uid: r.dst})"));
        assert!(cypher.contains("CREATE (a)-[:FOLLOWS {since: r.since}]->(b)"));
    }

    #[test]
    fn rejects_identifier_with_backtick() {
        let m = RowMapping::Node {
            label: "Bad`Label".into(),
            id_column: None,
            id_property: None,
            properties: vec![],
        };
        assert!(m.to_cypher().is_err());
    }
}
