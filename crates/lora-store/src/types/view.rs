//! Borrowed views of stored nodes and relationships.
//!
//! [`GraphStorage::with_node`](crate::GraphStorage::with_node) and
//! [`with_relationship`](crate::GraphStorage::with_relationship) hand a
//! reader one of these instead of a `&NodeRecord` / `&RelationshipRecord`.
//! A view says what an entity contains without saying how the backend
//! keeps it, so a backend may store records as structs, as encoded bytes
//! or on disk and still serve reads without building a record first.
//!
//! Views are `Copy` and borrow from the store for the duration of the
//! closure they are passed to. Every backend can make one from a record
//! (`NodeRef::from(&record)`), which is what the default trait methods do.

use std::sync::Arc;

use super::{
    Labels, Name, NodeId, NodeRecord, Properties, PropertyValue, RelationshipId, RelationshipRecord,
};

/// A borrowed property value.
///
/// Scalars are carried by value and strings by slice. Every other kind
/// is reached through [`ValueRef::Other`].
#[derive(Clone, Copy, Debug)]
pub enum ValueRef<'a> {
    Null,
    Bool(bool),
    Int(i64),
    Float(f64),
    String(&'a str),
    /// A value of any other kind: binary, list, map, temporal, point or
    /// vector. Never one of the kinds above.
    Other(&'a PropertyValue),
}

impl<'a> ValueRef<'a> {
    /// An owned copy of the value.
    pub fn to_owned(self) -> PropertyValue {
        match self {
            ValueRef::Null => PropertyValue::Null,
            ValueRef::Bool(v) => PropertyValue::Bool(v),
            ValueRef::Int(v) => PropertyValue::Int(v),
            ValueRef::Float(v) => PropertyValue::Float(v),
            ValueRef::String(v) => PropertyValue::String(v.to_owned()),
            ValueRef::Other(v) => v.clone(),
        }
    }

    pub fn is_null(self) -> bool {
        matches!(self, ValueRef::Null)
    }

    pub fn as_str(self) -> Option<&'a str> {
        match self {
            ValueRef::String(v) => Some(v),
            _ => None,
        }
    }
}

impl<'a> From<&'a PropertyValue> for ValueRef<'a> {
    #[inline]
    fn from(value: &'a PropertyValue) -> Self {
        match value {
            PropertyValue::Null => ValueRef::Null,
            PropertyValue::Bool(v) => ValueRef::Bool(*v),
            PropertyValue::Int(v) => ValueRef::Int(*v),
            PropertyValue::Float(v) => ValueRef::Float(*v),
            PropertyValue::String(v) => ValueRef::String(v),
            other => ValueRef::Other(other),
        }
    }
}

impl PartialEq<PropertyValue> for ValueRef<'_> {
    /// Same answer as comparing the owned values.
    fn eq(&self, other: &PropertyValue) -> bool {
        match (*self, other) {
            (ValueRef::Null, PropertyValue::Null) => true,
            (ValueRef::Bool(a), PropertyValue::Bool(b)) => a == *b,
            (ValueRef::Int(a), PropertyValue::Int(b)) => a == *b,
            (ValueRef::Float(a), PropertyValue::Float(b)) => a == *b,
            (ValueRef::String(a), PropertyValue::String(b)) => a == b,
            (ValueRef::Other(a), b) => a == b,
            _ => false,
        }
    }
}

impl PartialEq for ValueRef<'_> {
    fn eq(&self, other: &Self) -> bool {
        match (*self, *other) {
            (ValueRef::Null, ValueRef::Null) => true,
            (ValueRef::Bool(a), ValueRef::Bool(b)) => a == b,
            (ValueRef::Int(a), ValueRef::Int(b)) => a == b,
            (ValueRef::Float(a), ValueRef::Float(b)) => a == b,
            (ValueRef::String(a), ValueRef::String(b)) => a == b,
            (ValueRef::Other(a), ValueRef::Other(b)) => a == b,
            _ => false,
        }
    }
}

/// A borrowed property bag: keys in sorted order, each with its value.
#[derive(Clone, Copy)]
pub struct PropsRef<'a> {
    map: &'a Properties,
}

impl<'a> PropsRef<'a> {
    #[inline]
    pub fn get(self, key: &str) -> Option<ValueRef<'a>> {
        self.map.get(key).map(ValueRef::from)
    }

    #[inline]
    pub fn contains_key(self, key: &str) -> bool {
        self.map.contains_key(key)
    }

    #[inline]
    pub fn len(self) -> usize {
        self.map.len()
    }

    #[inline]
    pub fn is_empty(self) -> bool {
        self.map.is_empty()
    }

    /// `(key, value)` pairs in key order.
    pub fn iter(self) -> PropsIter<'a> {
        PropsIter {
            inner: self.map.iter(),
        }
    }

    /// Keys in sorted order.
    pub fn keys(self) -> impl ExactSizeIterator<Item = &'a str> + 'a {
        self.map.keys().map(|k| &**k)
    }

    /// Keys in sorted order, as the interned buffers the store shares.
    pub fn shared_keys(self) -> impl ExactSizeIterator<Item = Arc<str>> + 'a {
        self.map.keys().cloned()
    }

    /// An owned copy of the bag.
    pub fn to_owned(self) -> Properties {
        self.map.clone()
    }
}

impl<'a> From<&'a Properties> for PropsRef<'a> {
    #[inline]
    fn from(map: &'a Properties) -> Self {
        Self { map }
    }
}

impl<'a> From<&PropsRef<'a>> for PropsRef<'a> {
    #[inline]
    fn from(props: &PropsRef<'a>) -> Self {
        *props
    }
}

/// Iterator over a [`PropsRef`], in key order.
pub struct PropsIter<'a> {
    inner: super::property_map::Iter<'a>,
}

impl<'a> Iterator for PropsIter<'a> {
    type Item = (&'a str, ValueRef<'a>);

    #[inline]
    fn next(&mut self) -> Option<Self::Item> {
        self.inner.next().map(|(k, v)| (&**k, ValueRef::from(v)))
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        self.inner.size_hint()
    }
}

impl ExactSizeIterator for PropsIter<'_> {}

impl<'a> IntoIterator for PropsRef<'a> {
    type Item = (&'a str, ValueRef<'a>);
    type IntoIter = PropsIter<'a>;

    fn into_iter(self) -> PropsIter<'a> {
        self.iter()
    }
}

impl<'a> IntoIterator for &PropsRef<'a> {
    type Item = (&'a str, ValueRef<'a>);
    type IntoIter = PropsIter<'a>;

    fn into_iter(self) -> PropsIter<'a> {
        self.iter()
    }
}

impl std::fmt::Debug for PropsRef<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_map().entries(self.iter()).finish()
    }
}

/// A borrowed label set, in the order the labels were added.
#[derive(Clone, Copy)]
pub struct LabelsRef<'a> {
    labels: &'a [Name],
}

impl<'a> LabelsRef<'a> {
    #[inline]
    pub fn has(self, label: &str) -> bool {
        self.labels.iter().any(|l| l.as_str() == label)
    }

    #[inline]
    pub fn len(self) -> usize {
        self.labels.len()
    }

    #[inline]
    pub fn is_empty(self) -> bool {
        self.labels.is_empty()
    }

    pub fn iter(self) -> impl ExactSizeIterator<Item = &'a str> + Clone + 'a {
        self.labels.iter().map(Name::as_str)
    }

    pub fn to_strings(self) -> Vec<String> {
        self.labels.iter().map(String::from).collect()
    }

    /// An owned copy of the set.
    pub fn to_owned(self) -> Labels {
        self.labels.iter().cloned().collect()
    }
}

impl<'a> From<&'a Labels> for LabelsRef<'a> {
    #[inline]
    fn from(labels: &'a Labels) -> Self {
        Self {
            labels: labels.as_slice(),
        }
    }
}

impl<'a> From<&LabelsRef<'a>> for LabelsRef<'a> {
    #[inline]
    fn from(labels: &LabelsRef<'a>) -> Self {
        *labels
    }
}

impl<'a> From<&'a [Name]> for LabelsRef<'a> {
    #[inline]
    fn from(labels: &'a [Name]) -> Self {
        Self { labels }
    }
}

impl std::fmt::Debug for LabelsRef<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_list().entries(self.iter()).finish()
    }
}

/// A borrowed node.
#[derive(Clone, Copy, Debug)]
pub struct NodeRef<'a> {
    record: &'a NodeRecord,
}

impl<'a> NodeRef<'a> {
    #[inline]
    pub fn id(self) -> NodeId {
        self.record.id
    }

    #[inline]
    pub fn labels(self) -> LabelsRef<'a> {
        LabelsRef::from(&self.record.labels)
    }

    #[inline]
    pub fn has_label(self, label: &str) -> bool {
        self.record.labels.has(label)
    }

    #[inline]
    pub fn properties(self) -> PropsRef<'a> {
        PropsRef::from(&self.record.properties)
    }

    #[inline]
    pub fn property(self, key: &str) -> Option<ValueRef<'a>> {
        self.record.properties.get(key).map(ValueRef::from)
    }

    /// An owned copy of the node.
    pub fn to_record(self) -> NodeRecord {
        self.record.clone()
    }
}

impl<'a> From<&'a NodeRecord> for NodeRef<'a> {
    #[inline]
    fn from(record: &'a NodeRecord) -> Self {
        Self { record }
    }
}

/// A borrowed relationship.
#[derive(Clone, Copy, Debug)]
pub struct RelRef<'a> {
    record: &'a RelationshipRecord,
}

impl<'a> RelRef<'a> {
    #[inline]
    pub fn id(self) -> RelationshipId {
        self.record.id
    }

    #[inline]
    pub fn src(self) -> NodeId {
        self.record.src
    }

    #[inline]
    pub fn dst(self) -> NodeId {
        self.record.dst
    }

    #[inline]
    pub fn rel_type(self) -> &'a str {
        self.record.rel_type.as_str()
    }

    #[inline]
    pub fn properties(self) -> PropsRef<'a> {
        PropsRef::from(&self.record.properties)
    }

    #[inline]
    pub fn property(self, key: &str) -> Option<ValueRef<'a>> {
        self.record.properties.get(key).map(ValueRef::from)
    }

    /// The endpoint that is not `node_id`, if `node_id` is an endpoint. A
    /// self-loop returns `node_id`.
    pub fn other_node(self, node_id: NodeId) -> Option<NodeId> {
        self.record.other_node(node_id)
    }

    /// An owned copy of the relationship.
    pub fn to_record(self) -> RelationshipRecord {
        self.record.clone()
    }
}

impl<'a> From<&'a RelationshipRecord> for RelRef<'a> {
    #[inline]
    fn from(record: &'a RelationshipRecord) -> Self {
        Self { record }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> NodeRecord {
        let mut properties = Properties::new();
        properties.insert(crate::intern("name"), PropertyValue::String("Ada".into()));
        properties.insert(crate::intern("age"), PropertyValue::Int(36));
        properties.insert(
            crate::intern("tags"),
            PropertyValue::List(vec![PropertyValue::Int(1)]),
        );
        NodeRecord {
            id: 7,
            labels: ["Person", "Admin"].into(),
            properties,
        }
    }

    #[test]
    fn a_node_view_reads_what_the_record_holds() {
        let record = sample();
        let node = NodeRef::from(&record);
        assert_eq!(node.id(), 7);
        assert!(node.has_label("Admin") && !node.has_label("Nope"));
        assert_eq!(
            node.labels().iter().collect::<Vec<_>>(),
            ["Person", "Admin"]
        );
        assert_eq!(node.labels().to_owned(), record.labels);
        assert_eq!(node.property("age"), Some(ValueRef::Int(36)));
        assert_eq!(
            node.property("name").and_then(ValueRef::as_str),
            Some("Ada")
        );
        assert!(node.property("missing").is_none());
        assert_eq!(
            node.properties().keys().collect::<Vec<_>>(),
            ["age", "name", "tags"]
        );
        assert_eq!(node.properties().to_owned(), record.properties);
        assert_eq!(node.to_record(), record);
    }

    #[test]
    fn value_views_round_trip_and_compare_like_owned_values() {
        let record = sample();
        for (key, value) in record.properties.iter() {
            let view = ValueRef::from(value);
            assert_eq!(&view.to_owned(), value, "{key}");
            assert!(view == *value);
            for (_, other) in record.properties.iter() {
                assert_eq!(view == *other, value == other);
                assert_eq!(view == ValueRef::from(other), value == other);
            }
        }
        assert!(matches!(
            ValueRef::from(&PropertyValue::List(Vec::new())),
            ValueRef::Other(_)
        ));
    }
}
