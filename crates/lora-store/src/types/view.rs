//! Borrowed views of stored nodes and relationships.
//!
//! [`GraphStorage::with_node`](crate::GraphStorage::with_node) and
//! [`with_relationship`](crate::GraphStorage::with_relationship) hand a
//! reader one of these instead of a `&NodeRecord` / `&RelationshipRecord`.
//! A view says what an entity contains without saying how the backend
//! keeps it. The in-memory store keeps records as encoded bytes and its
//! views read them in place; any backend can also make a view from a
//! record (`NodeRef::from(&record)`), which is what the default trait
//! methods do.
//!
//! Views are `Copy` and borrow from the store for the duration of the
//! closure they are passed to.

use std::borrow::Cow;
use std::sync::Arc;

use super::{
    Labels, Name, NodeId, NodeRecord, Properties, PropertyValue, RelationshipId, RelationshipRecord,
};
use crate::encoded::{
    EncodedOther, StoredLabels, StoredNode, StoredProps, StoredPropsIter, StoredRel, StoredValue,
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
    Other(OtherValue<'a>),
}

/// A stored value of a kind [`ValueRef`] does not carry inline.
///
/// [`OtherValue::get`] yields the value: borrowed when the backend holds
/// it as a `PropertyValue`, decoded when it holds bytes.
#[derive(Clone, Copy)]
pub struct OtherValue<'a> {
    repr: OtherRepr<'a>,
}

#[derive(Clone, Copy)]
enum OtherRepr<'a> {
    Value(&'a PropertyValue),
    Encoded(EncodedOther<'a>),
}

impl<'a> OtherValue<'a> {
    pub fn get(self) -> Cow<'a, PropertyValue> {
        match self.repr {
            OtherRepr::Value(v) => Cow::Borrowed(v),
            OtherRepr::Encoded(v) => Cow::Owned(v.decode()),
        }
    }

    pub fn to_owned(self) -> PropertyValue {
        self.get().into_owned()
    }
}

impl std::fmt::Debug for OtherValue<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        std::fmt::Debug::fmt(&*self.get(), f)
    }
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
            ValueRef::Other(v) => v.to_owned(),
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

    #[inline]
    fn from_stored(value: StoredValue<'a>) -> Self {
        match value {
            StoredValue::Null => ValueRef::Null,
            StoredValue::Bool(v) => ValueRef::Bool(v),
            StoredValue::Int(v) => ValueRef::Int(v),
            StoredValue::Float(v) => ValueRef::Float(v),
            StoredValue::String(v) => ValueRef::String(v),
            StoredValue::Other(v) => ValueRef::Other(OtherValue {
                repr: OtherRepr::Encoded(v),
            }),
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
            other => ValueRef::Other(OtherValue {
                repr: OtherRepr::Value(other),
            }),
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
            (
                ValueRef::Other(_),
                PropertyValue::Null
                | PropertyValue::Bool(_)
                | PropertyValue::Int(_)
                | PropertyValue::Float(_)
                | PropertyValue::String(_),
            ) => false,
            (ValueRef::Other(a), b) => *a.get() == *b,
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
            (ValueRef::Other(a), ValueRef::Other(b)) => *a.get() == *b.get(),
            _ => false,
        }
    }
}

/// A borrowed property bag: keys in sorted order, each with its value.
#[derive(Clone, Copy)]
pub struct PropsRef<'a> {
    repr: PropsRepr<'a>,
}

#[derive(Clone, Copy)]
enum PropsRepr<'a> {
    Map(&'a Properties),
    Stored(StoredProps<'a>),
}

impl<'a> PropsRef<'a> {
    #[inline]
    pub fn get(self, key: &str) -> Option<ValueRef<'a>> {
        match self.repr {
            PropsRepr::Map(map) => map.get(key).map(ValueRef::from),
            PropsRepr::Stored(props) => props.get(key).map(ValueRef::from_stored),
        }
    }

    #[inline]
    pub fn contains_key(self, key: &str) -> bool {
        match self.repr {
            PropsRepr::Map(map) => map.contains_key(key),
            PropsRepr::Stored(props) => props.get(key).is_some(),
        }
    }

    #[inline]
    pub fn len(self) -> usize {
        match self.repr {
            PropsRepr::Map(map) => map.len(),
            PropsRepr::Stored(props) => props.len(),
        }
    }

    #[inline]
    pub fn is_empty(self) -> bool {
        self.len() == 0
    }

    /// `(key, value)` pairs in key order.
    pub fn iter(self) -> PropsIter<'a> {
        PropsIter {
            inner: match self.repr {
                PropsRepr::Map(map) => IterRepr::Map(map.iter()),
                PropsRepr::Stored(props) => IterRepr::Stored(props.iter()),
            },
        }
    }

    /// Keys in sorted order.
    pub fn keys(self) -> impl ExactSizeIterator<Item = &'a str> + 'a {
        self.iter().map(|(key, _)| key)
    }

    /// An owned copy of the bag.
    pub fn to_owned(self) -> Properties {
        match self.repr {
            PropsRepr::Map(map) => map.clone(),
            PropsRepr::Stored(props) => props.to_owned(),
        }
    }
}

impl<'a> From<&'a Properties> for PropsRef<'a> {
    #[inline]
    fn from(map: &'a Properties) -> Self {
        Self {
            repr: PropsRepr::Map(map),
        }
    }
}

impl<'a> From<&PropsRef<'a>> for PropsRef<'a> {
    #[inline]
    fn from(props: &PropsRef<'a>) -> Self {
        *props
    }
}

impl std::fmt::Debug for PropsRef<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_map().entries(self.iter()).finish()
    }
}

/// Iterator over a [`PropsRef`], in key order.
pub struct PropsIter<'a> {
    inner: IterRepr<'a>,
}

enum IterRepr<'a> {
    Map(super::property_map::Iter<'a>),
    Stored(StoredPropsIter<'a>),
}

impl<'a> PropsIter<'a> {
    /// The next pair, with the key as the shared buffer the store holds.
    fn next_shared(&mut self) -> Option<(&'a Arc<str>, ValueRef<'a>)> {
        match &mut self.inner {
            IterRepr::Map(iter) => iter.next().map(|(k, v)| (k, ValueRef::from(v))),
            IterRepr::Stored(iter) => iter
                .next()
                .map(|(k, v)| (k.as_arc(), ValueRef::from_stored(v))),
        }
    }
}

impl<'a> Iterator for PropsIter<'a> {
    type Item = (&'a str, ValueRef<'a>);

    #[inline]
    fn next(&mut self) -> Option<Self::Item> {
        self.next_shared().map(|(k, v)| (&**k, v))
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        match &self.inner {
            IterRepr::Map(iter) => iter.size_hint(),
            IterRepr::Stored(iter) => iter.size_hint(),
        }
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

/// A borrowed label set, in the order the labels were added.
#[derive(Clone, Copy)]
pub struct LabelsRef<'a> {
    repr: LabelsRepr<'a>,
}

#[derive(Clone, Copy)]
enum LabelsRepr<'a> {
    Names(&'a [Name]),
    Stored(StoredLabels<'a>),
}

impl<'a> LabelsRef<'a> {
    #[inline]
    pub fn has(self, label: &str) -> bool {
        match self.repr {
            LabelsRepr::Names(names) => names.iter().any(|l| l.as_str() == label),
            LabelsRepr::Stored(labels) => labels.has(label),
        }
    }

    #[inline]
    pub fn len(self) -> usize {
        match self.repr {
            LabelsRepr::Names(names) => names.len(),
            LabelsRepr::Stored(labels) => labels.len(),
        }
    }

    #[inline]
    pub fn is_empty(self) -> bool {
        self.len() == 0
    }

    /// The labels as interned names.
    pub fn names(self) -> impl ExactSizeIterator<Item = &'a Name> + Clone + 'a {
        let (names, stored) = match self.repr {
            LabelsRepr::Names(names) => (Some(names.iter()), None),
            LabelsRepr::Stored(labels) => (None, Some(labels.iter())),
        };
        LabelNames { names, stored }
    }

    pub fn iter(self) -> impl ExactSizeIterator<Item = &'a str> + Clone + 'a {
        self.names().map(Name::as_str)
    }

    pub fn to_strings(self) -> Vec<String> {
        self.names().map(String::from).collect()
    }

    /// An owned copy of the set.
    pub fn to_owned(self) -> Labels {
        self.names().cloned().collect()
    }
}

/// Either representation's name iterator, without boxing.
#[derive(Clone)]
struct LabelNames<A, B> {
    names: Option<A>,
    stored: Option<B>,
}

impl<'a, A, B> Iterator for LabelNames<A, B>
where
    A: Iterator<Item = &'a Name>,
    B: Iterator<Item = &'a Name>,
{
    type Item = &'a Name;

    #[inline]
    fn next(&mut self) -> Option<&'a Name> {
        match (&mut self.names, &mut self.stored) {
            (Some(iter), _) => iter.next(),
            (_, Some(iter)) => iter.next(),
            _ => None,
        }
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        match (&self.names, &self.stored) {
            (Some(iter), _) => iter.size_hint(),
            (_, Some(iter)) => iter.size_hint(),
            _ => (0, Some(0)),
        }
    }
}

impl<'a, A, B> ExactSizeIterator for LabelNames<A, B>
where
    A: ExactSizeIterator<Item = &'a Name>,
    B: ExactSizeIterator<Item = &'a Name>,
{
}

impl<'a> From<&'a Labels> for LabelsRef<'a> {
    #[inline]
    fn from(labels: &'a Labels) -> Self {
        Self {
            repr: LabelsRepr::Names(labels.as_slice()),
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
        Self {
            repr: LabelsRepr::Names(labels),
        }
    }
}

impl std::fmt::Debug for LabelsRef<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_list().entries(self.iter()).finish()
    }
}

/// A borrowed node.
#[derive(Clone, Copy)]
pub struct NodeRef<'a> {
    repr: NodeRepr<'a>,
}

#[derive(Clone, Copy)]
enum NodeRepr<'a> {
    Record(&'a NodeRecord),
    Stored(StoredNode<'a>),
}

impl<'a> NodeRef<'a> {
    #[inline]
    pub(crate) fn stored(node: StoredNode<'a>) -> Self {
        Self {
            repr: NodeRepr::Stored(node),
        }
    }

    #[inline]
    pub fn id(self) -> NodeId {
        match self.repr {
            NodeRepr::Record(record) => record.id,
            NodeRepr::Stored(node) => node.id,
        }
    }

    #[inline]
    pub fn labels(self) -> LabelsRef<'a> {
        match self.repr {
            NodeRepr::Record(record) => LabelsRef::from(&record.labels),
            NodeRepr::Stored(node) => LabelsRef {
                repr: LabelsRepr::Stored(node.labels()),
            },
        }
    }

    #[inline]
    pub fn has_label(self, label: &str) -> bool {
        self.labels().has(label)
    }

    #[inline]
    pub fn properties(self) -> PropsRef<'a> {
        match self.repr {
            NodeRepr::Record(record) => PropsRef::from(&record.properties),
            NodeRepr::Stored(node) => PropsRef {
                repr: PropsRepr::Stored(node.properties()),
            },
        }
    }

    #[inline]
    pub fn property(self, key: &str) -> Option<ValueRef<'a>> {
        self.properties().get(key)
    }

    /// An owned copy of the node.
    pub fn to_record(self) -> NodeRecord {
        match self.repr {
            NodeRepr::Record(record) => record.clone(),
            NodeRepr::Stored(node) => node.to_record(),
        }
    }
}

impl<'a> From<&'a NodeRecord> for NodeRef<'a> {
    #[inline]
    fn from(record: &'a NodeRecord) -> Self {
        Self {
            repr: NodeRepr::Record(record),
        }
    }
}

impl std::fmt::Debug for NodeRef<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("NodeRef")
            .field("id", &self.id())
            .field("labels", &self.labels())
            .field("properties", &self.properties())
            .finish()
    }
}

/// A borrowed relationship.
#[derive(Clone, Copy)]
pub struct RelRef<'a> {
    repr: RelRepr<'a>,
}

#[derive(Clone, Copy)]
enum RelRepr<'a> {
    Record(&'a RelationshipRecord),
    Stored(StoredRel<'a>),
}

impl<'a> RelRef<'a> {
    #[inline]
    pub(crate) fn stored(rel: StoredRel<'a>) -> Self {
        Self {
            repr: RelRepr::Stored(rel),
        }
    }

    #[inline]
    pub fn id(self) -> RelationshipId {
        match self.repr {
            RelRepr::Record(record) => record.id,
            RelRepr::Stored(rel) => rel.id,
        }
    }

    #[inline]
    pub fn src(self) -> NodeId {
        match self.repr {
            RelRepr::Record(record) => record.src,
            RelRepr::Stored(rel) => rel.src,
        }
    }

    #[inline]
    pub fn dst(self) -> NodeId {
        match self.repr {
            RelRepr::Record(record) => record.dst,
            RelRepr::Stored(rel) => rel.dst,
        }
    }

    /// The relationship type as an interned name.
    #[inline]
    pub fn type_name(self) -> &'a Name {
        match self.repr {
            RelRepr::Record(record) => &record.rel_type,
            RelRepr::Stored(rel) => rel.rel_type(),
        }
    }

    #[inline]
    pub fn rel_type(self) -> &'a str {
        self.type_name().as_str()
    }

    #[inline]
    pub fn properties(self) -> PropsRef<'a> {
        match self.repr {
            RelRepr::Record(record) => PropsRef::from(&record.properties),
            RelRepr::Stored(rel) => PropsRef {
                repr: PropsRepr::Stored(rel.properties()),
            },
        }
    }

    #[inline]
    pub fn property(self, key: &str) -> Option<ValueRef<'a>> {
        self.properties().get(key)
    }

    /// The endpoint that is not `node_id`, if `node_id` is an endpoint. A
    /// self-loop returns `node_id`.
    pub fn other_node(self, node_id: NodeId) -> Option<NodeId> {
        let (src, dst) = (self.src(), self.dst());
        if src == node_id {
            Some(dst)
        } else if dst == node_id {
            Some(src)
        } else {
            None
        }
    }

    /// An owned copy of the relationship.
    pub fn to_record(self) -> RelationshipRecord {
        match self.repr {
            RelRepr::Record(record) => record.clone(),
            RelRepr::Stored(rel) => rel.to_record(),
        }
    }
}

impl<'a> From<&'a RelationshipRecord> for RelRef<'a> {
    #[inline]
    fn from(record: &'a RelationshipRecord) -> Self {
        Self {
            repr: RelRepr::Record(record),
        }
    }
}

impl std::fmt::Debug for RelRef<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RelRef")
            .field("id", &self.id())
            .field("src", &self.src())
            .field("dst", &self.dst())
            .field("rel_type", &self.rel_type())
            .field("properties", &self.properties())
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dict::Dicts;
    use crate::encoded::{encode_node, encode_rel, StoredNode, StoredRel};

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

    fn check_node(node: NodeRef<'_>, record: &NodeRecord) {
        assert_eq!(node.id(), 7);
        assert!(node.has_label("Admin") && !node.has_label("Nope"));
        assert_eq!(
            node.labels().iter().collect::<Vec<_>>(),
            ["Person", "Admin"]
        );
        assert_eq!(node.labels().len(), 2);
        assert_eq!(node.labels().to_owned(), record.labels);
        assert_eq!(node.property("age"), Some(ValueRef::Int(36)));
        assert_eq!(
            node.property("name").and_then(ValueRef::as_str),
            Some("Ada")
        );
        assert!(node.property("missing").is_none());
        assert!(node.properties().contains_key("tags"));
        assert_eq!(node.properties().len(), 3);
        assert_eq!(
            node.properties().keys().collect::<Vec<_>>(),
            ["age", "name", "tags"]
        );
        assert_eq!(node.properties().to_owned(), record.properties);
        assert_eq!(&node.to_record(), record);
        for (key, value) in record.properties.iter() {
            let view = node.property(key).unwrap();
            assert_eq!(&view.to_owned(), value, "{key}");
            for (_, other) in record.properties.iter() {
                assert_eq!(view == *other, value == other);
                assert_eq!(view == ValueRef::from(other), value == other);
            }
        }
        assert!(matches!(node.property("tags"), Some(ValueRef::Other(_))));
    }

    #[test]
    fn a_node_view_reads_the_same_from_a_record_and_from_stored_bytes() {
        let record = sample();
        check_node(NodeRef::from(&record), &record);

        let mut dicts = Dicts::default();
        let blob = encode_node(&record, &mut dicts);
        check_node(NodeRef::stored(StoredNode::new(7, &blob, &dicts)), &record);
    }

    #[test]
    fn a_relationship_view_reads_the_same_from_both_forms() {
        let mut properties = Properties::new();
        properties.insert(crate::intern("since"), PropertyValue::Int(2020));
        let record = RelationshipRecord {
            id: 3,
            src: 10,
            dst: 20,
            rel_type: "KNOWS".into(),
            properties,
        };
        let mut dicts = Dicts::default();
        let blob = encode_rel(&record, &mut dicts);
        let stored = RelRef::stored(StoredRel::new(3, &blob, &dicts));
        for rel in [RelRef::from(&record), stored] {
            assert_eq!((rel.id(), rel.src(), rel.dst()), (3, 10, 20));
            assert_eq!(rel.rel_type(), "KNOWS");
            assert_eq!(rel.property("since"), Some(ValueRef::Int(2020)));
            assert_eq!(rel.other_node(10), Some(20));
            assert_eq!(rel.other_node(99), None);
            assert_eq!(rel.to_record(), record);
        }
    }
}
