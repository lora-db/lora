//! Core data types: coordinate-type tag, storage variant, and the
//! `LoraVector` struct itself.

use std::fmt;

/// Maximum dimension accepted by LoraDB's `vector(...)` constructor.
pub const MAX_VECTOR_DIMENSION: usize = 4096;

/// Canonical coordinate type for a vector.
///
/// The external tag names (`FLOAT64`, `FLOAT32`, `INTEGER`, `INTEGER32`,
/// `INTEGER16`, `INTEGER8`) are the serialization labels used by every
/// binding. Aliases (`FLOAT`, `INT`, `INT64`, `INTEGER64`, `INT32`,
/// `INT16`, `INT8`, `SIGNED INTEGER`) resolve to these canonical variants
/// at construction time and are not reported back in output.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, serde::Serialize, serde::Deserialize,
)]
pub enum VectorCoordinateType {
    Float64,
    Float32,
    Integer64,
    Integer32,
    Integer16,
    Integer8,
}

impl VectorCoordinateType {
    /// Canonical label emitted on the wire (tagged value `coordinateType`
    /// field). Lowercase aliases and the multi-word `SIGNED INTEGER`
    /// alias are accepted on input via [`parse`](Self::parse), but the
    /// output is always one of these six tags.
    pub fn as_str(self) -> &'static str {
        match self {
            VectorCoordinateType::Float64 => "FLOAT64",
            VectorCoordinateType::Float32 => "FLOAT32",
            VectorCoordinateType::Integer64 => "INTEGER",
            VectorCoordinateType::Integer32 => "INTEGER32",
            VectorCoordinateType::Integer16 => "INTEGER16",
            VectorCoordinateType::Integer8 => "INTEGER8",
        }
    }

    /// Parse a coordinate type from a user-supplied string. Accepts every
    /// alias documented in `vector()` / binding helpers; returns `None`
    /// when the name is unrecognised. Comparison is case-insensitive and
    /// collapses runs of whitespace so `SIGNED INTEGER` and `signed
    /// integer` both resolve.
    pub fn parse(name: &str) -> Option<Self> {
        let mut parts = name.split_whitespace();
        let first = parts.next()?;
        let second = parts.next();
        if parts.next().is_some() {
            return None;
        }

        if let Some(second) = second {
            return (first.eq_ignore_ascii_case("SIGNED")
                && second.eq_ignore_ascii_case("INTEGER"))
            .then_some(VectorCoordinateType::Integer64);
        }

        match first {
            // `FLOAT` and `FLOAT64` are the two spellings the public
            // `vector()` syntax accepts. `DOUBLE` is not part of the
            // public surface; we reject it so typos surface as a clear
            // "unknown coordinate type" instead of silently mapping to
            // FLOAT64.
            value if value.eq_ignore_ascii_case("FLOAT") => Some(VectorCoordinateType::Float64),
            value if value.eq_ignore_ascii_case("FLOAT64") => Some(VectorCoordinateType::Float64),
            value if value.eq_ignore_ascii_case("FLOAT32") => Some(VectorCoordinateType::Float32),
            value if value.eq_ignore_ascii_case("INTEGER") => Some(VectorCoordinateType::Integer64),
            value if value.eq_ignore_ascii_case("INT") => Some(VectorCoordinateType::Integer64),
            value if value.eq_ignore_ascii_case("INT64") => Some(VectorCoordinateType::Integer64),
            value if value.eq_ignore_ascii_case("INTEGER64") => {
                Some(VectorCoordinateType::Integer64)
            }
            value if value.eq_ignore_ascii_case("INTEGER32") => {
                Some(VectorCoordinateType::Integer32)
            }
            value if value.eq_ignore_ascii_case("INT32") => Some(VectorCoordinateType::Integer32),
            value if value.eq_ignore_ascii_case("INTEGER16") => {
                Some(VectorCoordinateType::Integer16)
            }
            value if value.eq_ignore_ascii_case("INT16") => Some(VectorCoordinateType::Integer16),
            value if value.eq_ignore_ascii_case("INTEGER8") => Some(VectorCoordinateType::Integer8),
            value if value.eq_ignore_ascii_case("INT8") => Some(VectorCoordinateType::Integer8),
            _ => None,
        }
    }

    /// True for `FLOAT` / `FLOAT32` / `FLOAT64`.
    pub fn is_float(self) -> bool {
        matches!(
            self,
            VectorCoordinateType::Float64 | VectorCoordinateType::Float32
        )
    }
}

impl fmt::Display for VectorCoordinateType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Internal storage for a vector. One variant per supported coordinate
/// type; dimension is implicit in the inner `Vec`'s length.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum VectorValues {
    Float64(Vec<f64>),
    Float32(Vec<f32>),
    Integer64(Vec<i64>),
    Integer32(Vec<i32>),
    Integer16(Vec<i16>),
    Integer8(Vec<i8>),
}

impl VectorValues {
    pub fn coordinate_type(&self) -> VectorCoordinateType {
        match self {
            VectorValues::Float64(_) => VectorCoordinateType::Float64,
            VectorValues::Float32(_) => VectorCoordinateType::Float32,
            VectorValues::Integer64(_) => VectorCoordinateType::Integer64,
            VectorValues::Integer32(_) => VectorCoordinateType::Integer32,
            VectorValues::Integer16(_) => VectorCoordinateType::Integer16,
            VectorValues::Integer8(_) => VectorCoordinateType::Integer8,
        }
    }

    pub fn len(&self) -> usize {
        match self {
            VectorValues::Float64(v) => v.len(),
            VectorValues::Float32(v) => v.len(),
            VectorValues::Integer64(v) => v.len(),
            VectorValues::Integer32(v) => v.len(),
            VectorValues::Integer16(v) => v.len(),
            VectorValues::Integer8(v) => v.len(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Return coordinate `index` widened/narrowed to `f32`, matching the
    /// arithmetic precision used by the vector math helpers.
    pub(crate) fn f32_at(&self, index: usize) -> Option<f32> {
        match self {
            VectorValues::Float64(v) => v.get(index).map(|x| *x as f32),
            VectorValues::Float32(v) => v.get(index).copied(),
            VectorValues::Integer64(v) => v.get(index).map(|x| *x as f32),
            VectorValues::Integer32(v) => v.get(index).map(|x| *x as f32),
            VectorValues::Integer16(v) => v.get(index).map(|x| *x as f32),
            VectorValues::Integer8(v) => v.get(index).map(|x| *x as f32),
        }
    }

    /// Lossless conversion of every coordinate to `f64`. Used by every
    /// vector-math function so the implementations can share one
    /// f32-precision accumulator irrespective of the underlying storage.
    pub fn as_f64_vec(&self) -> Vec<f64> {
        match self {
            VectorValues::Float64(v) => v.clone(),
            VectorValues::Float32(v) => v.iter().map(|x| *x as f64).collect(),
            VectorValues::Integer64(v) => v.iter().map(|x| *x as f64).collect(),
            VectorValues::Integer32(v) => v.iter().map(|x| *x as f64).collect(),
            VectorValues::Integer16(v) => v.iter().map(|x| *x as f64).collect(),
            VectorValues::Integer8(v) => v.iter().map(|x| *x as f64).collect(),
        }
    }

    /// Convert every coordinate to `i64`, truncating fractional parts for
    /// float-backed vectors. Matches the semantics required by
    /// `toIntegerList(vector)`.
    pub fn to_i64_vec(&self) -> Vec<i64> {
        match self {
            VectorValues::Float64(v) => v.iter().map(|x| *x as i64).collect(),
            VectorValues::Float32(v) => v.iter().map(|x| *x as i64).collect(),
            VectorValues::Integer64(v) => v.clone(),
            VectorValues::Integer32(v) => v.iter().map(|x| *x as i64).collect(),
            VectorValues::Integer16(v) => v.iter().map(|x| *x as i64).collect(),
            VectorValues::Integer8(v) => v.iter().map(|x| *x as i64).collect(),
        }
    }
}

/// A first-class VECTOR value.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct LoraVector {
    pub dimension: usize,
    pub values: VectorValues,
}

impl LoraVector {
    /// Total-order comparison key. Sorting vectors is mostly meaningful
    /// for tie-breaking inside `ORDER BY` — the key orders first by
    /// coordinate type tag, then by dimension, then by the coordinates
    /// rendered as `f64` (matches `as_f64_vec`). Callers that need a
    /// stable key for DISTINCT/grouping should use `to_key_string`.
    pub fn coordinate_type(&self) -> VectorCoordinateType {
        self.values.coordinate_type()
    }

    /// Canonical string form used for grouping / DISTINCT / UNION keys,
    /// and for the fallback sort comparator. Not meant for user display.
    pub fn to_key_string(&self) -> String {
        let mut out = String::new();
        out.push_str(self.coordinate_type().as_str());
        out.push('|');
        out.push_str(&self.dimension.to_string());
        out.push('|');
        let vals = self.values.as_f64_vec();
        for (i, v) in vals.iter().enumerate() {
            if i > 0 {
                out.push(',');
            }
            // Use `{:?}` so NaN is encoded distinctly from ±Inf — mirrors
            // the strategy used by GroupValueKey for `LoraValue::Float`.
            out.push_str(&format!("{v:?}"));
        }
        out
    }
}

impl fmt::Display for LoraVector {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "vector(")?;
        f.write_str("[")?;
        let values = self.values.as_f64_vec();
        for (i, v) in values.iter().enumerate() {
            if i > 0 {
                f.write_str(", ")?;
            }
            if self.coordinate_type().is_float() {
                write!(f, "{v}")?;
            } else {
                write!(f, "{}", *v as i64)?;
            }
        }
        f.write_str("], ")?;
        write!(
            f,
            "{}, {})",
            self.dimension,
            self.coordinate_type().as_str()
        )
    }
}
