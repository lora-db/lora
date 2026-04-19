use lora_analyzer::symbols::VarId;
use lora_store::{
    LoraDate, LoraDateTime, LoraDuration, LoraLocalDateTime, LoraLocalTime, LoraPoint, LoraTime,
    NodeId, PropertyValue, RelationshipId,
};

/// A materialised path: alternating node/relationship IDs.
/// nodes.len() == rels.len() + 1
#[derive(Debug, Clone, PartialEq)]
pub struct LoraPath {
    pub nodes: Vec<NodeId>,
    pub rels: Vec<RelationshipId>,
}
use serde::ser::{SerializeMap, SerializeSeq};
use serde::{Serialize, Serializer};
use std::collections::BTreeMap;

#[derive(Debug, Clone, PartialEq)]
pub enum LoraValue {
    Null,
    Bool(bool),
    Int(i64),
    Float(f64),
    String(String),
    List(Vec<LoraValue>),
    Map(BTreeMap<String, LoraValue>),
    Node(NodeId),
    Relationship(RelationshipId),
    Path(LoraPath),
    Date(LoraDate),
    Time(LoraTime),
    LocalTime(LoraLocalTime),
    DateTime(LoraDateTime),
    LocalDateTime(LoraLocalDateTime),
    Duration(LoraDuration),
    Point(LoraPoint),
}

impl LoraValue {
    pub fn is_truthy(&self) -> bool {
        match self {
            LoraValue::Null => false,
            LoraValue::Bool(v) => *v,
            _ => true,
        }
    }

    pub fn as_i64(&self) -> Option<i64> {
        match self {
            LoraValue::Int(v) => Some(*v),
            _ => None,
        }
    }

    pub fn as_f64(&self) -> Option<f64> {
        match self {
            LoraValue::Int(v) => Some(*v as f64),
            LoraValue::Float(v) => Some(*v),
            _ => None,
        }
    }
}

impl Serialize for LoraValue {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        match self {
            LoraValue::Null => serializer.serialize_unit(),
            LoraValue::Bool(v) => serializer.serialize_bool(*v),
            LoraValue::Int(v) => serializer.serialize_i64(*v),
            LoraValue::Float(v) => serializer.serialize_f64(*v),
            LoraValue::String(v) => serializer.serialize_str(v),

            LoraValue::List(values) => {
                let mut seq = serializer.serialize_seq(Some(values.len()))?;
                for value in values {
                    seq.serialize_element(value)?;
                }
                seq.end()
            }

            LoraValue::Map(map) => {
                let mut ser_map = serializer.serialize_map(Some(map.len()))?;
                for (k, v) in map {
                    ser_map.serialize_entry(k, v)?;
                }
                ser_map.end()
            }

            // These should ideally not reach output anymore if executor hydrates first.
            LoraValue::Node(id) => {
                let mut ser_map = serializer.serialize_map(Some(2))?;
                ser_map.serialize_entry("kind", "node")?;
                ser_map.serialize_entry("id", id)?;
                ser_map.end()
            }

            LoraValue::Relationship(id) => {
                let mut ser_map = serializer.serialize_map(Some(2))?;
                ser_map.serialize_entry("kind", "relationship")?;
                ser_map.serialize_entry("id", id)?;
                ser_map.end()
            }

            LoraValue::Path(path) => {
                let mut ser_map = serializer.serialize_map(Some(3))?;
                ser_map.serialize_entry("kind", "path")?;
                ser_map.serialize_entry("nodes", &path.nodes)?;
                ser_map.serialize_entry("rels", &path.rels)?;
                ser_map.end()
            }

            LoraValue::Date(d) => serializer.serialize_str(&d.to_string()),
            LoraValue::Time(t) => serializer.serialize_str(&t.to_string()),
            LoraValue::LocalTime(t) => serializer.serialize_str(&t.to_string()),
            LoraValue::DateTime(dt) => serializer.serialize_str(&dt.to_string()),
            LoraValue::LocalDateTime(dt) => serializer.serialize_str(&dt.to_string()),
            LoraValue::Duration(dur) => serializer.serialize_str(&dur.to_string()),
            LoraValue::Point(p) => {
                let len = if p.z.is_some() { 4 } else { 3 };
                let mut m = serializer.serialize_map(Some(len))?;
                m.serialize_entry("srid", &p.srid)?;
                m.serialize_entry("x", &p.x)?;
                m.serialize_entry("y", &p.y)?;
                if let Some(z) = p.z {
                    m.serialize_entry("z", &z)?;
                }
                m.end()
            }
        }
    }
}

impl From<PropertyValue> for LoraValue {
    fn from(value: PropertyValue) -> Self {
        match value {
            PropertyValue::Null => LoraValue::Null,
            PropertyValue::Bool(v) => LoraValue::Bool(v),
            PropertyValue::Int(v) => LoraValue::Int(v),
            PropertyValue::Float(v) => LoraValue::Float(v),
            PropertyValue::String(v) => LoraValue::String(v),
            PropertyValue::List(values) => {
                LoraValue::List(values.into_iter().map(LoraValue::from).collect())
            }
            PropertyValue::Map(map) => LoraValue::Map(
                map.into_iter()
                    .map(|(k, v)| (k, LoraValue::from(v)))
                    .collect(),
            ),
            PropertyValue::Date(d) => LoraValue::Date(d),
            PropertyValue::Time(t) => LoraValue::Time(t),
            PropertyValue::LocalTime(t) => LoraValue::LocalTime(t),
            PropertyValue::DateTime(dt) => LoraValue::DateTime(dt),
            PropertyValue::LocalDateTime(dt) => LoraValue::LocalDateTime(dt),
            PropertyValue::Duration(dur) => LoraValue::Duration(dur),
            PropertyValue::Point(p) => LoraValue::Point(p),
        }
    }
}

/// Build a `LoraValue` from a borrowed `PropertyValue` in a single walk. Lets
/// callers that already hold `&PropertyValue` (property lookups on borrowed
/// records) skip the `prop.clone().into()` double-traversal.
impl From<&PropertyValue> for LoraValue {
    fn from(value: &PropertyValue) -> Self {
        match value {
            PropertyValue::Null => LoraValue::Null,
            PropertyValue::Bool(v) => LoraValue::Bool(*v),
            PropertyValue::Int(v) => LoraValue::Int(*v),
            PropertyValue::Float(v) => LoraValue::Float(*v),
            PropertyValue::String(v) => LoraValue::String(v.clone()),
            PropertyValue::List(values) => {
                LoraValue::List(values.iter().map(LoraValue::from).collect())
            }
            PropertyValue::Map(map) => LoraValue::Map(
                map.iter()
                    .map(|(k, v)| (k.clone(), LoraValue::from(v)))
                    .collect(),
            ),
            PropertyValue::Date(d) => LoraValue::Date(d.clone()),
            PropertyValue::Time(t) => LoraValue::Time(t.clone()),
            PropertyValue::LocalTime(t) => LoraValue::LocalTime(t.clone()),
            PropertyValue::DateTime(dt) => LoraValue::DateTime(dt.clone()),
            PropertyValue::LocalDateTime(dt) => LoraValue::LocalDateTime(dt.clone()),
            PropertyValue::Duration(dur) => LoraValue::Duration(dur.clone()),
            PropertyValue::Point(p) => LoraValue::Point(p.clone()),
        }
    }
}

impl From<LoraValue> for PropertyValue {
    fn from(value: LoraValue) -> Self {
        match value {
            LoraValue::Null => PropertyValue::Null,
            LoraValue::Bool(v) => PropertyValue::Bool(v),
            LoraValue::Int(v) => PropertyValue::Int(v),
            LoraValue::Float(v) => PropertyValue::Float(v),
            LoraValue::String(v) => PropertyValue::String(v),
            LoraValue::List(values) => {
                PropertyValue::List(values.into_iter().map(PropertyValue::from).collect())
            }
            LoraValue::Map(map) => PropertyValue::Map(
                map.into_iter()
                    .map(|(k, v)| (k, PropertyValue::from(v)))
                    .collect(),
            ),
            LoraValue::Node(id) => PropertyValue::String(format!("node:{id}")),
            LoraValue::Relationship(id) => PropertyValue::String(format!("rel:{id}")),
            LoraValue::Path(_) => PropertyValue::Null,
            LoraValue::Date(d) => PropertyValue::Date(d),
            LoraValue::Time(t) => PropertyValue::Time(t),
            LoraValue::LocalTime(t) => PropertyValue::LocalTime(t),
            LoraValue::DateTime(dt) => PropertyValue::DateTime(dt),
            LoraValue::LocalDateTime(dt) => PropertyValue::LocalDateTime(dt),
            LoraValue::Duration(dur) => PropertyValue::Duration(dur),
            LoraValue::Point(p) => PropertyValue::Point(p),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
struct RowEntry {
    /// `None` means "use the fallback `_{key}` lazily". This avoids allocating
    /// a String for every anonymous variable on the insert hot path.
    name: Option<String>,
    value: LoraValue,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Row {
    values: BTreeMap<VarId, RowEntry>,
}

impl Serialize for Row {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut ser_map = serializer.serialize_map(Some(self.values.len()))?;
        for (key, entry) in &self.values {
            match &entry.name {
                Some(name) => ser_map.serialize_entry(name.as_str(), &entry.value)?,
                None => {
                    let fallback = format!("_{key}");
                    ser_map.serialize_entry(fallback.as_str(), &entry.value)?;
                }
            }
        }
        ser_map.end()
    }
}

impl Row {
    pub fn new() -> Self {
        Self {
            values: BTreeMap::new(),
        }
    }

    pub fn get(&self, key: VarId) -> Option<&LoraValue> {
        self.values.get(&key).map(|entry| &entry.value)
    }

    /// Returns the column name for `key`, generating the `_{key}` fallback
    /// on demand for entries inserted without an explicit name.
    pub fn get_name(&self, key: VarId) -> Option<String> {
        self.values.get(&key).map(|entry| match &entry.name {
            Some(n) => n.clone(),
            None => format!("_{key}"),
        })
    }

    pub fn insert(&mut self, key: VarId, value: LoraValue) {
        // Preserve any previously-set explicit name when overwriting an entry;
        // otherwise leave name as None so the fallback is produced lazily.
        use std::collections::btree_map::Entry;
        match self.values.entry(key) {
            Entry::Occupied(mut e) => e.get_mut().value = value,
            Entry::Vacant(e) => {
                e.insert(RowEntry { name: None, value });
            }
        }
    }

    pub fn insert_named(&mut self, key: VarId, name: impl Into<String>, value: LoraValue) {
        self.values.insert(
            key,
            RowEntry {
                name: Some(name.into()),
                value,
            },
        );
    }

    pub fn extend_from(&mut self, other: &Row) {
        for (k, v) in &other.values {
            self.values.insert(*k, v.clone());
        }
    }

    pub fn iter(&self) -> impl Iterator<Item = (&VarId, &LoraValue)> {
        self.values.iter().map(|(k, entry)| (k, &entry.value))
    }

    /// Iterate `(key, name, value)`. The name is a `Cow`: borrowed when an
    /// explicit name was stored, and owned (lazily formatted as `_{key}`) for
    /// entries inserted via the anonymous `insert()` path.
    pub fn iter_named(
        &self,
    ) -> impl Iterator<Item = (&VarId, std::borrow::Cow<'_, str>, &LoraValue)> {
        self.values.iter().map(|(k, entry)| {
            let name: std::borrow::Cow<'_, str> = match &entry.name {
                Some(n) => std::borrow::Cow::Borrowed(n.as_str()),
                None => std::borrow::Cow::Owned(format!("_{k}")),
            };
            (k, name, &entry.value)
        })
    }

    /// Consume the row and yield owned `(VarId, name, LoraValue)` triples.
    /// Used by hydrate_row to avoid cloning values on the projection hot path.
    pub fn into_iter_named(self) -> impl Iterator<Item = (VarId, String, LoraValue)> {
        self.values.into_iter().map(|(k, entry)| {
            (
                k,
                entry.name.unwrap_or_else(|| format!("_{k}")),
                entry.value,
            )
        })
    }

    pub fn len(&self) -> usize {
        self.values.len()
    }

    pub fn is_empty(&self) -> bool {
        self.values.is_empty()
    }

    pub fn contains_key(&self, key: VarId) -> bool {
        self.values.contains_key(&key)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResultFormat {
    Rows,
    RowArrays,
    Graph,
    Combined,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ExecuteOptions {
    pub format: ResultFormat,
}

impl Default for ExecuteOptions {
    fn default() -> Self {
        Self {
            format: ResultFormat::Graph,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(untagged)]
pub enum QueryResult {
    Rows(RowsResult),
    RowArrays(RowArraysResult),
    Graph(GraphResult),
    Combined(CombinedResult),
}

#[derive(Debug, Clone, Serialize)]
pub struct RowsResult {
    pub rows: Vec<Row>,
}

#[derive(Debug, Clone, Serialize)]
pub struct RowArraysResult {
    pub columns: Vec<String>,
    pub rows: Vec<Vec<LoraValue>>,
}

#[derive(Debug, Clone, Serialize)]
pub struct GraphResult {
    pub graph: HydratedGraph,
}

#[derive(Debug, Clone, Serialize)]
pub struct CombinedResult {
    pub columns: Vec<String>,
    pub data: Vec<CombinedRow>,
    pub graph: HydratedGraph,
}

#[derive(Debug, Clone, Serialize)]
pub struct CombinedRow {
    pub row: Vec<LoraValue>,
}

#[derive(Debug, Clone, Serialize, Default)]
pub struct HydratedGraph {
    pub nodes: Vec<HydratedNode>,
    pub relationships: Vec<HydratedRelationship>,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct HydratedNode {
    pub id: i64,
    pub labels: Vec<String>,
    pub properties: BTreeMap<String, LoraValue>,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct HydratedRelationship {
    pub id: i64,
    #[serde(rename = "startId")]
    pub start_id: i64,
    #[serde(rename = "endId")]
    pub end_id: i64,
    #[serde(rename = "type")]
    pub rel_type: String,
    pub properties: BTreeMap<String, LoraValue>,
}

pub fn project_rows(rows: Vec<Row>, options: ExecuteOptions) -> QueryResult {
    match options.format {
        ResultFormat::Rows => QueryResult::Rows(RowsResult { rows }),

        ResultFormat::RowArrays => {
            let columns = infer_columns(&rows);
            let projected_rows = rows.iter().map(|row| row_to_array(row, &columns)).collect();

            QueryResult::RowArrays(RowArraysResult {
                columns,
                rows: projected_rows,
            })
        }

        ResultFormat::Graph => QueryResult::Graph(GraphResult {
            graph: collect_hydrated_graph(&rows),
        }),

        ResultFormat::Combined => {
            let columns = infer_columns(&rows);
            let data = rows
                .iter()
                .map(|row| CombinedRow {
                    row: row_to_array(row, &columns),
                })
                .collect();

            QueryResult::Combined(CombinedResult {
                columns,
                data,
                graph: collect_hydrated_graph(&rows),
            })
        }
    }
}

fn infer_columns(rows: &[Row]) -> Vec<String> {
    rows.first()
        .map(|row| {
            row.iter_named()
                .map(|(_, name, _)| name.into_owned())
                .collect::<Vec<_>>()
        })
        .unwrap_or_default()
}

fn row_to_array(row: &Row, columns: &[String]) -> Vec<LoraValue> {
    // Row entry count is small; a linear scan per column avoids allocating
    // owned names into an intermediate lookup map.
    columns
        .iter()
        .map(|col| {
            row.iter_named()
                .find(|(_, name, _)| name.as_ref() == col.as_str())
                .map(|(_, _, v)| v.clone())
                .unwrap_or(LoraValue::Null)
        })
        .collect()
}

fn collect_hydrated_graph(rows: &[Row]) -> HydratedGraph {
    let mut nodes = BTreeMap::<i64, HydratedNode>::new();
    let mut relationships = BTreeMap::<i64, HydratedRelationship>::new();

    for row in rows {
        for (_, _, value) in row.iter_named() {
            collect_graph_from_value(value, &mut nodes, &mut relationships);
        }
    }

    HydratedGraph {
        nodes: nodes.into_values().collect(),
        relationships: relationships.into_values().collect(),
    }
}

fn collect_graph_from_value(
    value: &LoraValue,
    nodes: &mut BTreeMap<i64, HydratedNode>,
    relationships: &mut BTreeMap<i64, HydratedRelationship>,
) {
    match value {
        LoraValue::List(values) => {
            for value in values {
                collect_graph_from_value(value, nodes, relationships);
            }
        }

        LoraValue::Map(map) => {
            if let Some(node) = try_as_hydrated_node(map) {
                nodes.entry(node.id).or_insert(node);
                return;
            }

            if let Some(rel) = try_as_hydrated_relationship(map) {
                relationships.entry(rel.id).or_insert(rel);
                return;
            }

            for value in map.values() {
                collect_graph_from_value(value, nodes, relationships);
            }
        }

        _ => {}
    }
}

fn try_as_hydrated_node(map: &BTreeMap<String, LoraValue>) -> Option<HydratedNode> {
    let id = match map.get("id")? {
        LoraValue::Int(v) => *v,
        _ => return None,
    };

    let labels = match map.get("labels")? {
        LoraValue::List(values) => values
            .iter()
            .map(|v| match v {
                LoraValue::String(s) => Some(s.clone()),
                _ => None,
            })
            .collect::<Option<Vec<_>>>()?,
        _ => return None,
    };

    let properties = match map.get("properties")? {
        LoraValue::Map(props) => props.clone(),
        _ => return None,
    };

    Some(HydratedNode {
        id,
        labels,
        properties,
    })
}

fn try_as_hydrated_relationship(map: &BTreeMap<String, LoraValue>) -> Option<HydratedRelationship> {
    match map.get("kind") {
        Some(LoraValue::String(kind)) if kind == "relationship" => {}
        _ => return None,
    }

    let id = match map.get("id")? {
        LoraValue::Int(v) => *v,
        _ => return None,
    };

    let start_id = match map.get("startId").or_else(|| map.get("src"))? {
        LoraValue::Int(v) => *v,
        _ => return None,
    };

    let end_id = match map.get("endId").or_else(|| map.get("dst"))? {
        LoraValue::Int(v) => *v,
        _ => return None,
    };

    let rel_type = match map.get("type")? {
        LoraValue::String(s) => s.clone(),
        _ => return None,
    };

    let properties = match map.get("properties")? {
        LoraValue::Map(props) => props.clone(),
        _ => return None,
    };

    Some(HydratedRelationship {
        id,
        start_id,
        end_id,
        rel_type,
        properties,
    })
}
