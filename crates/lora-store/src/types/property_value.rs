//! The polymorphic property value shared by node and relationship
//! records.
//!
//! `PropertyValue` is the single discriminated union every property in
//! the graph carries. Each variant either wraps a primitive (`Bool`,
//! `Int`, …) or a richer value type defined in a sibling module
//! (`Binary`, `Date`, `Point`, `Vector`, …). Keeping the enum in its
//! own file makes adding a new variant a one-file edit and avoids
//! pulling every richer-value module into the graph-records file.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

use super::binary::LoraBinary;
use super::spatial::LoraPoint;
use super::temporal::{
    LoraDate, LoraDateTime, LoraDuration, LoraLocalDateTime, LoraLocalTime, LoraTime,
};
use super::vector::LoraVector;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum PropertyValue {
    Null,
    Bool(bool),
    Int(i64),
    Float(f64),
    String(String),
    Binary(LoraBinary),
    List(Vec<PropertyValue>),
    Map(BTreeMap<String, PropertyValue>),
    Date(LoraDate),
    Time(LoraTime),
    LocalTime(LoraLocalTime),
    DateTime(LoraDateTime),
    LocalDateTime(LoraLocalDateTime),
    Duration(LoraDuration),
    Point(LoraPoint),
    Vector(LoraVector),
}
