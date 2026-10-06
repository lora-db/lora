//! Shared label and relationship-type names.
//!
//! A graph has few distinct labels and relationship types but stores one
//! per record. [`Name`] is an interned `Arc<str>`: every record carrying
//! `Person` points at the same bytes, so a record costs a refcount bump
//! instead of a heap `String`. [`Labels`] keeps a node's labels inline
//! for the common single-label case.
//!
//! Both serialize exactly like the `String` / `Vec<String>` they replace.

use std::borrow::Borrow;
use std::fmt;
use std::ops::Deref;
use std::sync::Arc;

use serde::{Deserialize, Deserializer, Serialize, Serializer};
use smallvec::SmallVec;

use crate::intern::{intern, intern_owned};

/// An interned label or relationship-type name.
#[derive(Clone, Eq, PartialOrd, Ord)]
pub struct Name(Arc<str>);

impl Name {
    pub fn new(name: &str) -> Self {
        Self(intern(name))
    }

    #[inline]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// The shared buffer, for callers that keep `Arc<str>` keys.
    #[inline]
    pub fn as_arc(&self) -> &Arc<str> {
        &self.0
    }
}

impl PartialEq for Name {
    #[inline]
    fn eq(&self, other: &Self) -> bool {
        // Equal names normally share one interned buffer, so the pointer
        // check settles most comparisons; the bytes decide the rest.
        Arc::ptr_eq(&self.0, &other.0) || *self.0 == *other.0
    }
}

impl std::hash::Hash for Name {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        // Same as `str`, so `Borrow<str>` lookups work.
        self.0.hash(state)
    }
}

impl Deref for Name {
    type Target = str;

    #[inline]
    fn deref(&self) -> &str {
        &self.0
    }
}

impl AsRef<str> for Name {
    #[inline]
    fn as_ref(&self) -> &str {
        &self.0
    }
}

impl Borrow<str> for Name {
    #[inline]
    fn borrow(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for Name {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(&*self.0, f)
    }
}

impl fmt::Display for Name {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl From<&str> for Name {
    fn from(name: &str) -> Self {
        Self::new(name)
    }
}

impl From<String> for Name {
    fn from(name: String) -> Self {
        Self(intern_owned(name))
    }
}

impl From<&String> for Name {
    fn from(name: &String) -> Self {
        Self::new(name)
    }
}

impl From<Name> for String {
    fn from(name: Name) -> Self {
        name.as_str().to_owned()
    }
}

impl From<&Name> for String {
    fn from(name: &Name) -> Self {
        name.as_str().to_owned()
    }
}

impl PartialEq<str> for Name {
    #[inline]
    fn eq(&self, other: &str) -> bool {
        &*self.0 == other
    }
}

impl PartialEq<&str> for Name {
    #[inline]
    fn eq(&self, other: &&str) -> bool {
        &*self.0 == *other
    }
}

impl PartialEq<String> for Name {
    #[inline]
    fn eq(&self, other: &String) -> bool {
        &*self.0 == other.as_str()
    }
}

impl PartialEq<Name> for str {
    #[inline]
    fn eq(&self, other: &Name) -> bool {
        self == &*other.0
    }
}

impl PartialEq<Name> for &str {
    #[inline]
    fn eq(&self, other: &Name) -> bool {
        *self == &*other.0
    }
}

impl PartialEq<Name> for String {
    #[inline]
    fn eq(&self, other: &Name) -> bool {
        self.as_str() == &*other.0
    }
}

impl Serialize for Name {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for Name {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        String::deserialize(deserializer).map(Name::from)
    }
}

/// A node's labels, in the order they were added.
///
/// One label is stored inline; more spill to the heap.
#[derive(Clone, Default, PartialEq, Eq)]
pub struct Labels(SmallVec<Name, 1>);

impl Labels {
    pub fn new() -> Self {
        Self::default()
    }

    #[inline]
    pub fn as_slice(&self) -> &[Name] {
        self.0.as_slice()
    }

    #[inline]
    pub fn has(&self, label: &str) -> bool {
        self.0.iter().any(|l| l.as_str() == label)
    }

    pub fn push(&mut self, label: impl Into<Name>) {
        self.0.push(label.into());
    }

    pub fn retain(&mut self, mut keep: impl FnMut(&Name) -> bool) {
        self.0.retain(|l| keep(l));
    }

    /// The labels as `&str`s.
    pub fn strs(&self) -> impl ExactSizeIterator<Item = &str> + Clone + '_ {
        self.0.iter().map(Name::as_str)
    }

    /// Owned `String` copies, for callers that need `Vec<String>`.
    pub fn to_strings(&self) -> Vec<String> {
        self.0.iter().map(String::from).collect()
    }

    /// Heap bytes this set owns beyond its inline storage. The name
    /// buffers are shared and not counted.
    pub fn heap_bytes(&self) -> usize {
        if self.0.spilled() {
            self.0.capacity() * std::mem::size_of::<Name>()
        } else {
            0
        }
    }
}

impl Deref for Labels {
    type Target = [Name];

    #[inline]
    fn deref(&self) -> &[Name] {
        self.0.as_slice()
    }
}

impl fmt::Debug for Labels {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_list().entries(self.0.iter()).finish()
    }
}

impl<N: Into<Name>> FromIterator<N> for Labels {
    fn from_iter<I: IntoIterator<Item = N>>(iter: I) -> Self {
        Self(iter.into_iter().map(Into::into).collect())
    }
}

impl<N: Into<Name>> From<Vec<N>> for Labels {
    fn from(labels: Vec<N>) -> Self {
        labels.into_iter().collect()
    }
}

impl<N: Into<Name>, const K: usize> From<[N; K]> for Labels {
    fn from(labels: [N; K]) -> Self {
        labels.into_iter().collect()
    }
}

impl From<&[String]> for Labels {
    fn from(labels: &[String]) -> Self {
        labels.iter().collect()
    }
}

impl From<Labels> for Vec<String> {
    fn from(labels: Labels) -> Self {
        labels.to_strings()
    }
}

impl<'a> IntoIterator for &'a Labels {
    type Item = &'a Name;
    type IntoIter = std::slice::Iter<'a, Name>;

    fn into_iter(self) -> Self::IntoIter {
        self.0.iter()
    }
}

impl PartialEq<Vec<String>> for Labels {
    fn eq(&self, other: &Vec<String>) -> bool {
        self.0.len() == other.len() && self.0.iter().zip(other).all(|(a, b)| a == b)
    }
}

impl PartialEq<Labels> for Vec<String> {
    fn eq(&self, other: &Labels) -> bool {
        other == self
    }
}

impl<const N: usize> PartialEq<[&str; N]> for Labels {
    fn eq(&self, other: &[&str; N]) -> bool {
        self.0.len() == N && self.0.iter().zip(other).all(|(a, b)| a == b)
    }
}

impl PartialEq<Vec<&str>> for Labels {
    fn eq(&self, other: &Vec<&str>) -> bool {
        self.0.len() == other.len() && self.0.iter().zip(other).all(|(a, b)| a == b)
    }
}

impl Serialize for Labels {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_seq(self.0.iter())
    }
}

impl<'de> Deserialize<'de> for Labels {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Vec::<String>::deserialize(deserializer).map(Labels::from)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_share_a_buffer_and_compare_with_strings() {
        let a = Name::new("Person");
        let b = Name::from(String::from("Person"));
        assert!(Arc::ptr_eq(a.as_arc(), b.as_arc()));
        assert_eq!(a, "Person");
        assert_eq!(a, String::from("Person"));
        assert_eq!(String::from("Person"), a);
        assert_ne!(a, Name::new("City"));
    }

    #[test]
    fn labels_keep_order_and_round_trip_as_strings() {
        let labels: Labels = vec!["B".to_string(), "A".to_string()].into();
        assert_eq!(labels, ["B", "A"]);
        assert!(labels.has("A"));
        assert!(!labels.has("C"));
        assert_eq!(labels.to_strings(), vec!["B".to_string(), "A".to_string()]);
        let json = serde_json::to_string(&labels).unwrap();
        assert_eq!(json, r#"["B","A"]"#);
        let back: Labels = serde_json::from_str(&json).unwrap();
        assert_eq!(back, labels);
    }

    #[test]
    fn one_label_needs_no_heap() {
        let labels: Labels = ["Person"].into_iter().collect();
        assert_eq!(labels.heap_bytes(), 0);
        assert_eq!(std::mem::size_of::<Labels>(), 24);
    }
}
