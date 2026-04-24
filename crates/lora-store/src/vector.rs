//! First-class VECTOR value type.
//!
//! LoraDB VECTOR values are fixed-dimension, typed numeric coordinate
//! collections. A `LoraVector` can be stored directly as a node or
//! relationship property, returned through every binding, compared for
//! equality, and used as input to the built-in vector math functions
//! (`vector.similarity.cosine`, `vector.similarity.euclidean`,
//! `vector_distance`, `vector_norm`, `vector_dimension_count`,
//! `toIntegerList`, `toFloatList`).
//!
//! Vector indexes and approximate kNN are intentionally out of scope for
//! this pass — exhaustive search via `ORDER BY vector.similarity.*(…)
//! LIMIT k` works today; an index-backed variant is future work.

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
        let collapsed: String = name
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
            .to_ascii_uppercase();
        match collapsed.as_str() {
            // `FLOAT` and `FLOAT64` are the two spellings the public
            // `vector()` syntax accepts. `DOUBLE` is not part of the
            // public surface; we reject it so typos surface as a clear
            // "unknown coordinate type" instead of silently mapping to
            // FLOAT64.
            "FLOAT" | "FLOAT64" => Some(VectorCoordinateType::Float64),
            "FLOAT32" => Some(VectorCoordinateType::Float32),
            "INTEGER" | "INT" | "INT64" | "INTEGER64" | "SIGNED INTEGER" => {
                Some(VectorCoordinateType::Integer64)
            }
            "INTEGER32" | "INT32" => Some(VectorCoordinateType::Integer32),
            "INTEGER16" | "INT16" => Some(VectorCoordinateType::Integer16),
            "INTEGER8" | "INT8" => Some(VectorCoordinateType::Integer8),
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

// ---------------------------------------------------------------------------
// Construction
// ---------------------------------------------------------------------------

/// Error returned by [`LoraVector::try_new`]. Kept as a concrete enum so
/// the executor can render a single-line error message without inspecting
/// the underlying cause.
#[derive(Debug, Clone, PartialEq)]
pub enum VectorBuildError {
    InvalidDimension(i64),
    DimensionMismatch {
        expected: usize,
        got: usize,
    },
    NestedListNotAllowed,
    NonNumericCoordinate(String),
    NonFiniteCoordinate,
    OutOfRange {
        coordinate_type: VectorCoordinateType,
        value: String,
    },
    UnknownCoordinateType(String),
}

impl fmt::Display for VectorBuildError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            VectorBuildError::InvalidDimension(d) => {
                write!(
                    f,
                    "vector dimension must be between 1 and {MAX_VECTOR_DIMENSION}, got {d}"
                )
            }
            VectorBuildError::DimensionMismatch { expected, got } => write!(
                f,
                "vector value length {got} does not match declared dimension {expected}"
            ),
            VectorBuildError::NestedListNotAllowed => {
                write!(f, "vector coordinates cannot contain nested lists")
            }
            VectorBuildError::NonNumericCoordinate(kind) => {
                write!(f, "vector coordinates must be numeric, got {kind}")
            }
            VectorBuildError::NonFiniteCoordinate => {
                write!(f, "vector coordinates cannot be NaN or Infinity")
            }
            VectorBuildError::OutOfRange {
                coordinate_type,
                value,
            } => write!(
                f,
                "value {value} is out of range for coordinate type {}",
                coordinate_type.as_str()
            ),
            VectorBuildError::UnknownCoordinateType(name) => {
                write!(f, "unknown vector coordinate type '{name}'")
            }
        }
    }
}

impl std::error::Error for VectorBuildError {}

/// Raw numeric input for one coordinate before it has been coerced into
/// the destination coordinate type. Executors / binding layers feed
/// values through this enum so the coercion rules live in one place.
#[derive(Debug, Clone, Copy)]
pub enum RawCoordinate {
    Int(i64),
    Float(f64),
}

impl RawCoordinate {
    fn as_f64(self) -> f64 {
        match self {
            RawCoordinate::Int(v) => v as f64,
            RawCoordinate::Float(v) => v,
        }
    }
}

impl LoraVector {
    /// Build a vector from raw numeric coordinates, applying validation
    /// and coordinate-type coercion. Single entry point used by both
    /// `vector()` in Cypher and the binding-side constructors.
    pub fn try_new(
        raw: Vec<RawCoordinate>,
        dimension: i64,
        coordinate_type: VectorCoordinateType,
    ) -> Result<Self, VectorBuildError> {
        if dimension <= 0 || dimension as usize > MAX_VECTOR_DIMENSION {
            return Err(VectorBuildError::InvalidDimension(dimension));
        }
        let dim = dimension as usize;
        if raw.len() != dim {
            return Err(VectorBuildError::DimensionMismatch {
                expected: dim,
                got: raw.len(),
            });
        }

        for c in &raw {
            if let RawCoordinate::Float(v) = c {
                if !v.is_finite() {
                    return Err(VectorBuildError::NonFiniteCoordinate);
                }
            }
        }

        let values = match coordinate_type {
            VectorCoordinateType::Float64 => {
                VectorValues::Float64(raw.iter().map(|c| c.as_f64()).collect())
            }
            VectorCoordinateType::Float32 => {
                let mut out = Vec::with_capacity(dim);
                for c in &raw {
                    let v = c.as_f64();
                    if v.abs() > f32::MAX as f64 {
                        return Err(VectorBuildError::OutOfRange {
                            coordinate_type,
                            value: format!("{v}"),
                        });
                    }
                    out.push(v as f32);
                }
                VectorValues::Float32(out)
            }
            VectorCoordinateType::Integer64 => {
                let mut out = Vec::with_capacity(dim);
                for c in &raw {
                    out.push(coerce_to_int::<i64>(*c, coordinate_type)?);
                }
                VectorValues::Integer64(out)
            }
            VectorCoordinateType::Integer32 => {
                let mut out = Vec::with_capacity(dim);
                for c in &raw {
                    out.push(coerce_to_int::<i32>(*c, coordinate_type)?);
                }
                VectorValues::Integer32(out)
            }
            VectorCoordinateType::Integer16 => {
                let mut out = Vec::with_capacity(dim);
                for c in &raw {
                    out.push(coerce_to_int::<i16>(*c, coordinate_type)?);
                }
                VectorValues::Integer16(out)
            }
            VectorCoordinateType::Integer8 => {
                let mut out = Vec::with_capacity(dim);
                for c in &raw {
                    out.push(coerce_to_int::<i8>(*c, coordinate_type)?);
                }
                VectorValues::Integer8(out)
            }
        };

        Ok(LoraVector {
            dimension: dim,
            values,
        })
    }
}

/// Private helper: coerce a raw numeric coordinate into a specific signed
/// integer target. Float inputs truncate toward zero per LoraDB vector
/// coercion semantics; the result must fit in the target type or we
/// raise `OutOfRange`.
fn coerce_to_int<T>(
    raw: RawCoordinate,
    coordinate_type: VectorCoordinateType,
) -> Result<T, VectorBuildError>
where
    T: TryFrom<i64> + Copy,
{
    let as_i64 = match raw {
        RawCoordinate::Int(v) => v,
        RawCoordinate::Float(v) => {
            // `as i64` saturates on out-of-range floats, which would mask
            // overflow — do the check explicitly against the range of
            // i64 before truncating.
            if v > i64::MAX as f64 || v < i64::MIN as f64 {
                return Err(VectorBuildError::OutOfRange {
                    coordinate_type,
                    value: format!("{v}"),
                });
            }
            v.trunc() as i64
        }
    };

    T::try_from(as_i64).map_err(|_| VectorBuildError::OutOfRange {
        coordinate_type,
        value: as_i64.to_string(),
    })
}

/// Parse a string-form coordinate list, e.g. `"[1.05e+00, 0.123, 5]"`.
/// Used by `vector()` when `vectorValue` is a STRING.
pub fn parse_string_values(input: &str) -> Result<Vec<RawCoordinate>, VectorBuildError> {
    let trimmed = input.trim();
    if !trimmed.starts_with('[') || !trimmed.ends_with(']') {
        return Err(VectorBuildError::NonNumericCoordinate(
            "string must start with '[' and end with ']'".to_string(),
        ));
    }
    let body = &trimmed[1..trimmed.len() - 1];
    if body.trim().is_empty() {
        return Ok(Vec::new());
    }

    let mut out = Vec::new();
    for part in body.split(',') {
        let token = part.trim();
        if token.is_empty() {
            return Err(VectorBuildError::NonNumericCoordinate(
                "empty list entry".to_string(),
            ));
        }

        // Accept integer-looking tokens as Int so integer coordinate
        // types never go through float truncation unnecessarily.
        if let Ok(i) = token.parse::<i64>() {
            out.push(RawCoordinate::Int(i));
            continue;
        }
        match token.parse::<f64>() {
            Ok(f) if f.is_finite() => out.push(RawCoordinate::Float(f)),
            Ok(_) => return Err(VectorBuildError::NonFiniteCoordinate),
            Err(_) => {
                return Err(VectorBuildError::NonNumericCoordinate(format!(
                    "cannot parse '{token}'"
                )))
            }
        }
    }
    Ok(out)
}

// ---------------------------------------------------------------------------
// Vector math
// ---------------------------------------------------------------------------

/// Return Some(value) if both vectors have the same dimension; None if
/// they don't. Callers route the None branch to a query error so that
/// `vector_distance` / `vector.similarity.*` never silently return a
/// bogus number.
fn check_same_dim(a: &LoraVector, b: &LoraVector) -> Option<usize> {
    if a.dimension == b.dimension {
        Some(a.dimension)
    } else {
        None
    }
}

/// Raw cosine similarity in the range [-1, 1]. Returns `None` when
/// either vector has zero norm, since cosine is undefined in that case.
pub fn cosine_similarity_raw(a: &LoraVector, b: &LoraVector) -> Option<f64> {
    check_same_dim(a, b)?;
    // Use f32 arithmetic for LoraDB's vector similarity implementation,
    // then widen back to f64 for the result.
    let av: Vec<f32> = a
        .values
        .as_f64_vec()
        .into_iter()
        .map(|x| x as f32)
        .collect();
    let bv: Vec<f32> = b
        .values
        .as_f64_vec()
        .into_iter()
        .map(|x| x as f32)
        .collect();
    let mut dot = 0f32;
    let mut na = 0f32;
    let mut nb = 0f32;
    for (x, y) in av.iter().zip(bv.iter()) {
        dot += x * y;
        na += x * x;
        nb += y * y;
    }
    if na == 0.0 || nb == 0.0 {
        return None;
    }
    let denom = na.sqrt() * nb.sqrt();
    if denom == 0.0 {
        return None;
    }
    Some((dot / denom) as f64)
}

/// Cosine similarity squashed into [0, 1]. Matches the documented
/// `vector.similarity.cosine` behaviour.
pub fn cosine_similarity_bounded(a: &LoraVector, b: &LoraVector) -> Option<f64> {
    cosine_similarity_raw(a, b).map(|raw| ((raw + 1.0) / 2.0).clamp(0.0, 1.0))
}

/// Squared Euclidean distance (sum of squared differences). Uses f32
/// arithmetic to match LoraDB's vector function implementation.
pub fn euclidean_distance_squared(a: &LoraVector, b: &LoraVector) -> Option<f64> {
    check_same_dim(a, b)?;
    let av: Vec<f32> = a
        .values
        .as_f64_vec()
        .into_iter()
        .map(|x| x as f32)
        .collect();
    let bv: Vec<f32> = b
        .values
        .as_f64_vec()
        .into_iter()
        .map(|x| x as f32)
        .collect();
    let mut sum = 0f32;
    for (x, y) in av.iter().zip(bv.iter()) {
        let d = x - y;
        sum += d * d;
    }
    Some(sum as f64)
}

/// Euclidean (L2) distance.
pub fn euclidean_distance(a: &LoraVector, b: &LoraVector) -> Option<f64> {
    euclidean_distance_squared(a, b).map(f64::sqrt)
}

/// Manhattan (L1) distance.
pub fn manhattan_distance(a: &LoraVector, b: &LoraVector) -> Option<f64> {
    check_same_dim(a, b)?;
    let av = a.values.as_f64_vec();
    let bv = b.values.as_f64_vec();
    let mut sum = 0f32;
    for (x, y) in av.iter().zip(bv.iter()) {
        sum += ((*x as f32) - (*y as f32)).abs();
    }
    Some(sum as f64)
}

/// Hamming distance: count of positions where the two vectors differ.
pub fn hamming_distance(a: &LoraVector, b: &LoraVector) -> Option<f64> {
    check_same_dim(a, b)?;
    let av = a.values.as_f64_vec();
    let bv = b.values.as_f64_vec();
    let mut count = 0i64;
    for (x, y) in av.iter().zip(bv.iter()) {
        if (*x as f32) != (*y as f32) {
            count += 1;
        }
    }
    Some(count as f64)
}

/// Dot product (f32 arithmetic, widened back to f64).
pub fn dot_product(a: &LoraVector, b: &LoraVector) -> Option<f64> {
    check_same_dim(a, b)?;
    let av = a.values.as_f64_vec();
    let bv = b.values.as_f64_vec();
    let mut acc = 0f32;
    for (x, y) in av.iter().zip(bv.iter()) {
        acc += (*x as f32) * (*y as f32);
    }
    Some(acc as f64)
}

/// Euclidean (L2) norm.
pub fn euclidean_norm(v: &LoraVector) -> f64 {
    let values = v.values.as_f64_vec();
    let mut sum = 0f32;
    for x in &values {
        let x32 = *x as f32;
        sum += x32 * x32;
    }
    (sum.sqrt()) as f64
}

/// Manhattan (L1) norm.
pub fn manhattan_norm(v: &LoraVector) -> f64 {
    let values = v.values.as_f64_vec();
    let mut sum = 0f32;
    for x in &values {
        sum += (*x as f32).abs();
    }
    sum as f64
}

/// Similarity score derived from squared Euclidean distance: `1 / (1 +
/// d²)`. For the documented example where `distance² == 22`, this
/// yields `1 / 23 ≈ 0.043478`.
pub fn euclidean_similarity(a: &LoraVector, b: &LoraVector) -> Option<f64> {
    euclidean_distance_squared(a, b).map(|d2| 1.0 / (1.0 + d2))
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_coordinate_type_accepts_aliases() {
        assert_eq!(
            VectorCoordinateType::parse("INTEGER"),
            Some(VectorCoordinateType::Integer64)
        );
        assert_eq!(
            VectorCoordinateType::parse("int64"),
            Some(VectorCoordinateType::Integer64)
        );
        assert_eq!(
            VectorCoordinateType::parse("signed integer"),
            Some(VectorCoordinateType::Integer64)
        );
        assert_eq!(
            VectorCoordinateType::parse("  SIGNED    INTEGER "),
            Some(VectorCoordinateType::Integer64)
        );
        assert_eq!(
            VectorCoordinateType::parse("FLOAT"),
            Some(VectorCoordinateType::Float64)
        );
        assert_eq!(
            VectorCoordinateType::parse("float32"),
            Some(VectorCoordinateType::Float32)
        );
        assert_eq!(VectorCoordinateType::parse("bogus"), None);
    }

    #[test]
    fn try_new_rejects_zero_dim() {
        let err = LoraVector::try_new(vec![], 0, VectorCoordinateType::Float64).unwrap_err();
        assert!(matches!(err, VectorBuildError::InvalidDimension(0)));
    }

    #[test]
    fn try_new_rejects_over_max_dim() {
        let err = LoraVector::try_new(
            vec![RawCoordinate::Int(1); 1],
            (MAX_VECTOR_DIMENSION + 1) as i64,
            VectorCoordinateType::Float64,
        )
        .unwrap_err();
        assert!(matches!(err, VectorBuildError::InvalidDimension(_)));
    }

    #[test]
    fn try_new_rejects_dimension_mismatch() {
        let err = LoraVector::try_new(
            vec![RawCoordinate::Int(1)],
            2,
            VectorCoordinateType::Integer64,
        )
        .unwrap_err();
        assert!(matches!(
            err,
            VectorBuildError::DimensionMismatch {
                expected: 2,
                got: 1
            }
        ));
    }

    #[test]
    fn int8_overflow_errors() {
        let err = LoraVector::try_new(
            vec![RawCoordinate::Int(128)],
            1,
            VectorCoordinateType::Integer8,
        )
        .unwrap_err();
        assert!(matches!(err, VectorBuildError::OutOfRange { .. }));
    }

    #[test]
    fn float_to_int_truncates() {
        let v = LoraVector::try_new(
            vec![RawCoordinate::Float(1.9), RawCoordinate::Float(-1.9)],
            2,
            VectorCoordinateType::Integer64,
        )
        .unwrap();
        match v.values {
            VectorValues::Integer64(ref values) => assert_eq!(values, &[1, -1]),
            _ => panic!("expected Integer64"),
        }
    }

    #[test]
    fn int_to_float_is_allowed() {
        let v = LoraVector::try_new(
            vec![RawCoordinate::Int(3), RawCoordinate::Int(4)],
            2,
            VectorCoordinateType::Float32,
        )
        .unwrap();
        assert_eq!(v.values, VectorValues::Float32(vec![3.0, 4.0]));
    }

    #[test]
    fn parse_string_values_handles_scientific() {
        let parsed = parse_string_values("[1.05e+00, 0.123, 5]").unwrap();
        assert_eq!(parsed.len(), 3);
        match parsed[0] {
            RawCoordinate::Float(f) => assert!((f - 1.05).abs() < 1e-9),
            _ => panic!("expected float"),
        }
        match parsed[2] {
            RawCoordinate::Int(i) => assert_eq!(i, 5),
            _ => panic!("expected int"),
        }
    }

    #[test]
    fn cosine_similarity_is_bounded() {
        let a = LoraVector::try_new(
            vec![RawCoordinate::Int(1), RawCoordinate::Int(0)],
            2,
            VectorCoordinateType::Float32,
        )
        .unwrap();
        let b = LoraVector::try_new(
            vec![RawCoordinate::Int(1), RawCoordinate::Int(0)],
            2,
            VectorCoordinateType::Float32,
        )
        .unwrap();
        let sim = cosine_similarity_bounded(&a, &b).unwrap();
        assert!((sim - 1.0).abs() < 1e-6);
    }

    #[test]
    fn euclidean_similarity_matches_documented_example() {
        // Documented Euclidean similarity example:
        // d^2 = (4-2)^2 + (5-8)^2 + (6-3)^2 = 22
        // similarity = 1 / (1 + 22) ≈ 0.0434782
        let a = LoraVector::try_new(
            vec![
                RawCoordinate::Float(4.0),
                RawCoordinate::Float(5.0),
                RawCoordinate::Float(6.0),
            ],
            3,
            VectorCoordinateType::Float32,
        )
        .unwrap();
        let b = LoraVector::try_new(
            vec![
                RawCoordinate::Float(2.0),
                RawCoordinate::Float(8.0),
                RawCoordinate::Float(3.0),
            ],
            3,
            VectorCoordinateType::Float32,
        )
        .unwrap();
        let sim = euclidean_similarity(&a, &b).unwrap();
        assert!((sim - (1.0 / 23.0)).abs() < 1e-6, "got {sim}");
    }

    // ----------------------------------------------------------------------
    // Coordinate type alias coverage
    // ----------------------------------------------------------------------

    /// Small deterministic table mapping every accepted input form to its
    /// canonical variant. Keeps the alias list here exhaustive so adding a
    /// new alias needs a corresponding table row.
    #[test]
    fn parse_coordinate_type_every_alias() {
        use VectorCoordinateType::*;
        let cases: &[(&str, VectorCoordinateType)] = &[
            ("FLOAT", Float64),
            ("Float", Float64),
            ("float", Float64),
            ("FLOAT64", Float64),
            ("float64", Float64),
            ("FLOAT32", Float32),
            ("float32", Float32),
            ("INTEGER", Integer64),
            ("Integer", Integer64),
            ("integer", Integer64),
            ("INT", Integer64),
            ("int", Integer64),
            ("INT64", Integer64),
            ("int64", Integer64),
            ("INTEGER64", Integer64),
            ("SIGNED INTEGER", Integer64),
            ("signed integer", Integer64),
            ("Signed  Integer", Integer64),
            ("INTEGER32", Integer32),
            ("int32", Integer32),
            ("INT32", Integer32),
            ("INTEGER16", Integer16),
            ("INT16", Integer16),
            ("int16", Integer16),
            ("INTEGER8", Integer8),
            ("INT8", Integer8),
            ("int8", Integer8),
        ];
        for (input, expected) in cases {
            assert_eq!(
                VectorCoordinateType::parse(input),
                Some(*expected),
                "failed for input {input:?}"
            );
        }
    }

    #[test]
    fn parse_coordinate_type_rejects_unsupported_aliases() {
        for bogus in [
            "DOUBLE",
            "double",
            "REAL",
            "NUMBER",
            "BIGINT",
            "INT128",
            "FLOAT128",
            "UINT8",
            "UNSIGNED INTEGER",
            "BIT",
            "",
        ] {
            assert_eq!(
                VectorCoordinateType::parse(bogus),
                None,
                "should reject {bogus:?}"
            );
        }
    }

    #[test]
    fn parse_coordinate_type_is_whitespace_tolerant() {
        assert_eq!(
            VectorCoordinateType::parse("\tinteger\n"),
            Some(VectorCoordinateType::Integer64)
        );
        assert_eq!(
            VectorCoordinateType::parse("   INTEGER   "),
            Some(VectorCoordinateType::Integer64)
        );
    }

    // ----------------------------------------------------------------------
    // parse_string_values
    // ----------------------------------------------------------------------

    fn unwrap_float(raw: RawCoordinate) -> f64 {
        match raw {
            RawCoordinate::Float(f) => f,
            RawCoordinate::Int(i) => i as f64,
        }
    }

    fn unwrap_int(raw: RawCoordinate) -> i64 {
        match raw {
            RawCoordinate::Int(i) => i,
            RawCoordinate::Float(f) => panic!("expected Int, got Float({f})"),
        }
    }

    #[test]
    fn parse_string_values_accepts_negatives_and_whitespace() {
        let parsed = parse_string_values("  [ -1,  -2.5 ,   3 , -4.0e-2 ]  ").unwrap();
        assert_eq!(unwrap_int(parsed[0]), -1);
        assert!((unwrap_float(parsed[1]) + 2.5).abs() < 1e-9);
        assert_eq!(unwrap_int(parsed[2]), 3);
        assert!((unwrap_float(parsed[3]) + 0.04).abs() < 1e-12);
    }

    #[test]
    fn parse_string_values_accepts_signed_exponents() {
        let parsed = parse_string_values("[1e+10, 1e-10, -2.5e+3]").unwrap();
        assert!((unwrap_float(parsed[0]) - 1e10).abs() < 1.0);
        assert!((unwrap_float(parsed[1]) - 1e-10).abs() < 1e-20);
        assert!((unwrap_float(parsed[2]) + 2500.0).abs() < 1e-9);
    }

    #[test]
    fn parse_string_values_accepts_empty_brackets() {
        let parsed = parse_string_values("[]").unwrap();
        assert!(parsed.is_empty());
    }

    #[test]
    fn parse_string_values_rejects_missing_brackets() {
        assert!(parse_string_values("1, 2, 3").is_err());
        assert!(parse_string_values("[1, 2, 3").is_err());
        assert!(parse_string_values("1, 2, 3]").is_err());
    }

    #[test]
    fn parse_string_values_rejects_empty_entries() {
        assert!(parse_string_values("[1, , 3]").is_err());
        assert!(parse_string_values("[,1,2]").is_err());
        assert!(parse_string_values("[1,2,]").is_err());
        assert!(parse_string_values("[ , ]").is_err());
    }

    #[test]
    fn parse_string_values_rejects_non_numeric_tokens() {
        assert!(parse_string_values("[1, abc, 3]").is_err());
        assert!(parse_string_values("[true, false]").is_err());
        assert!(parse_string_values("[\"1\", \"2\"]").is_err());
    }

    #[test]
    fn parse_string_values_rejects_non_finite() {
        for bad in ["[NaN]", "[Infinity]", "[-Infinity]", "[1, NaN, 3]"] {
            assert!(parse_string_values(bad).is_err(), "should reject {bad:?}");
        }
    }

    // ----------------------------------------------------------------------
    // Dimension boundaries
    // ----------------------------------------------------------------------

    #[test]
    fn try_new_accepts_exactly_max_dimension() {
        let raw = vec![RawCoordinate::Int(0); MAX_VECTOR_DIMENSION];
        let v = LoraVector::try_new(
            raw,
            MAX_VECTOR_DIMENSION as i64,
            VectorCoordinateType::Integer8,
        )
        .expect("4096 should be accepted");
        assert_eq!(v.dimension, MAX_VECTOR_DIMENSION);
    }

    #[test]
    fn try_new_rejects_max_plus_one_dimension() {
        let err = LoraVector::try_new(
            vec![RawCoordinate::Int(0); MAX_VECTOR_DIMENSION + 1],
            (MAX_VECTOR_DIMENSION + 1) as i64,
            VectorCoordinateType::Integer8,
        )
        .unwrap_err();
        assert!(matches!(err, VectorBuildError::InvalidDimension(_)));
    }

    #[test]
    fn try_new_rejects_negative_dimension() {
        let err = LoraVector::try_new(vec![], -1, VectorCoordinateType::Integer64).unwrap_err();
        assert!(matches!(err, VectorBuildError::InvalidDimension(-1)));
    }

    // ----------------------------------------------------------------------
    // Integer min/max boundaries and overflow
    // ----------------------------------------------------------------------

    /// Table-driven min/max test: each entry supplies the coordinate type
    /// plus the min/max value that should fit and the just-out-of-range
    /// values that must overflow.
    #[test]
    fn integer_boundaries_round_trip() {
        let cases: &[(VectorCoordinateType, i64, i64, i64, i64)] = &[
            // (type,                        min,                    max,                    under,            over)
            (
                VectorCoordinateType::Integer8,
                i8::MIN as i64,
                i8::MAX as i64,
                i8::MIN as i64 - 1,
                i8::MAX as i64 + 1,
            ),
            (
                VectorCoordinateType::Integer16,
                i16::MIN as i64,
                i16::MAX as i64,
                i16::MIN as i64 - 1,
                i16::MAX as i64 + 1,
            ),
            (
                VectorCoordinateType::Integer32,
                i32::MIN as i64,
                i32::MAX as i64,
                i32::MIN as i64 - 1,
                i32::MAX as i64 + 1,
            ),
            (VectorCoordinateType::Integer64, i64::MIN, i64::MAX, 0, 0),
        ];
        for (ty, min, max, under, over) in cases {
            // min and max should succeed.
            LoraVector::try_new(vec![RawCoordinate::Int(*min)], 1, *ty)
                .unwrap_or_else(|e| panic!("{ty:?} min rejected: {e}"));
            LoraVector::try_new(vec![RawCoordinate::Int(*max)], 1, *ty)
                .unwrap_or_else(|e| panic!("{ty:?} max rejected: {e}"));

            // Integer64 has no out-of-range at the i64 level — skip.
            if *ty == VectorCoordinateType::Integer64 {
                continue;
            }

            let e = LoraVector::try_new(vec![RawCoordinate::Int(*under)], 1, *ty).unwrap_err();
            assert!(matches!(e, VectorBuildError::OutOfRange { .. }));
            let e = LoraVector::try_new(vec![RawCoordinate::Int(*over)], 1, *ty).unwrap_err();
            assert!(matches!(e, VectorBuildError::OutOfRange { .. }));
        }
    }

    #[test]
    fn float32_overflow_errors() {
        // A value that fits comfortably in f64 but overflows f32's max.
        let huge = (f32::MAX as f64) * 10.0;
        let err = LoraVector::try_new(
            vec![RawCoordinate::Float(huge)],
            1,
            VectorCoordinateType::Float32,
        )
        .unwrap_err();
        assert!(matches!(err, VectorBuildError::OutOfRange { .. }));
    }

    #[test]
    fn float_to_int_truncates_toward_zero() {
        // Both 1.9 and -1.9 truncate toward 0, not toward -inf.
        let v = LoraVector::try_new(
            vec![
                RawCoordinate::Float(1.9),
                RawCoordinate::Float(-1.9),
                RawCoordinate::Float(0.999),
                RawCoordinate::Float(-0.999),
            ],
            4,
            VectorCoordinateType::Integer8,
        )
        .unwrap();
        match v.values {
            VectorValues::Integer8(ref values) => assert_eq!(values, &[1i8, -1, 0, 0]),
            _ => panic!("expected Integer8"),
        }
    }

    #[test]
    fn float_out_of_range_i64_errors() {
        // An f64 well outside i64's range must error, not saturate.
        let err = LoraVector::try_new(
            vec![RawCoordinate::Float(f64::MAX)],
            1,
            VectorCoordinateType::Integer64,
        )
        .unwrap_err();
        assert!(matches!(err, VectorBuildError::OutOfRange { .. }));
    }

    #[test]
    fn non_finite_float_rejected_in_try_new() {
        for bad in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            let err = LoraVector::try_new(
                vec![RawCoordinate::Float(bad)],
                1,
                VectorCoordinateType::Float64,
            )
            .unwrap_err();
            assert!(matches!(err, VectorBuildError::NonFiniteCoordinate));
        }
    }

    // ----------------------------------------------------------------------
    // to_key_string
    // ----------------------------------------------------------------------

    #[test]
    fn to_key_string_distinguishes_coord_type_dim_and_values() {
        fn v(coord: VectorCoordinateType, vals: &[i64], dim: i64) -> LoraVector {
            LoraVector::try_new(
                vals.iter().map(|x| RawCoordinate::Int(*x)).collect(),
                dim,
                coord,
            )
            .unwrap()
        }

        // Different coord types with matching values must differ.
        let a = v(VectorCoordinateType::Integer64, &[1, 2, 3], 3);
        let b = v(VectorCoordinateType::Integer32, &[1, 2, 3], 3);
        assert_ne!(a.to_key_string(), b.to_key_string());

        // Different dimensions differ.
        let c = v(VectorCoordinateType::Integer64, &[1, 2], 2);
        assert_ne!(a.to_key_string(), c.to_key_string());

        // Different values differ.
        let d = v(VectorCoordinateType::Integer64, &[1, 2, 4], 3);
        assert_ne!(a.to_key_string(), d.to_key_string());

        // Identical keys match — used by DISTINCT / grouping.
        let a2 = v(VectorCoordinateType::Integer64, &[1, 2, 3], 3);
        assert_eq!(a.to_key_string(), a2.to_key_string());
    }

    // ----------------------------------------------------------------------
    // Math spot-checks (guard against silent regressions)
    // ----------------------------------------------------------------------

    #[test]
    fn cosine_orthogonal_is_zero_raw_and_half_bounded() {
        let a = LoraVector::try_new(
            vec![RawCoordinate::Int(1), RawCoordinate::Int(0)],
            2,
            VectorCoordinateType::Float32,
        )
        .unwrap();
        let b = LoraVector::try_new(
            vec![RawCoordinate::Int(0), RawCoordinate::Int(1)],
            2,
            VectorCoordinateType::Float32,
        )
        .unwrap();
        assert!((cosine_similarity_raw(&a, &b).unwrap()).abs() < 1e-6);
        assert!((cosine_similarity_bounded(&a, &b).unwrap() - 0.5).abs() < 1e-6);
    }

    #[test]
    fn cosine_opposite_is_neg_one_raw_and_zero_bounded() {
        let a = LoraVector::try_new(
            vec![RawCoordinate::Int(1), RawCoordinate::Int(0)],
            2,
            VectorCoordinateType::Float32,
        )
        .unwrap();
        let b = LoraVector::try_new(
            vec![RawCoordinate::Int(-1), RawCoordinate::Int(0)],
            2,
            VectorCoordinateType::Float32,
        )
        .unwrap();
        assert!((cosine_similarity_raw(&a, &b).unwrap() + 1.0).abs() < 1e-6);
        assert!(cosine_similarity_bounded(&a, &b).unwrap().abs() < 1e-6);
    }

    #[test]
    fn cosine_zero_vector_returns_none() {
        let zero = LoraVector::try_new(
            vec![RawCoordinate::Int(0), RawCoordinate::Int(0)],
            2,
            VectorCoordinateType::Float32,
        )
        .unwrap();
        let other = LoraVector::try_new(
            vec![RawCoordinate::Int(1), RawCoordinate::Int(0)],
            2,
            VectorCoordinateType::Float32,
        )
        .unwrap();
        assert!(cosine_similarity_raw(&zero, &other).is_none());
        assert!(cosine_similarity_bounded(&zero, &other).is_none());
    }

    #[test]
    fn distance_helpers_respect_dimension_mismatch() {
        let a = LoraVector::try_new(
            vec![RawCoordinate::Int(1), RawCoordinate::Int(0)],
            2,
            VectorCoordinateType::Float32,
        )
        .unwrap();
        let b = LoraVector::try_new(
            vec![
                RawCoordinate::Int(1),
                RawCoordinate::Int(0),
                RawCoordinate::Int(0),
            ],
            3,
            VectorCoordinateType::Float32,
        )
        .unwrap();
        assert!(euclidean_distance(&a, &b).is_none());
        assert!(euclidean_distance_squared(&a, &b).is_none());
        assert!(manhattan_distance(&a, &b).is_none());
        assert!(hamming_distance(&a, &b).is_none());
        assert!(dot_product(&a, &b).is_none());
    }

    #[test]
    fn manhattan_and_euclidean_norm_match_hand_computed() {
        // v = [3, 4, 0, -12] — L1 = 19, L2 = 13.
        let v = LoraVector::try_new(
            vec![
                RawCoordinate::Float(3.0),
                RawCoordinate::Float(4.0),
                RawCoordinate::Float(0.0),
                RawCoordinate::Float(-12.0),
            ],
            4,
            VectorCoordinateType::Float32,
        )
        .unwrap();
        assert!((manhattan_norm(&v) - 19.0).abs() < 1e-5);
        assert!((euclidean_norm(&v) - 13.0).abs() < 1e-5);
    }

    #[test]
    fn hamming_on_float_vectors_uses_f32_comparison() {
        // Both vectors store values that truncate to the same f32, so
        // hamming should report 0 mismatches — documents the f32 rule.
        let a = LoraVector::try_new(
            vec![RawCoordinate::Float(1.0), RawCoordinate::Float(2.0)],
            2,
            VectorCoordinateType::Float32,
        )
        .unwrap();
        let b = LoraVector::try_new(
            vec![RawCoordinate::Float(1.0), RawCoordinate::Float(2.0)],
            2,
            VectorCoordinateType::Float64,
        )
        .unwrap();
        assert!((hamming_distance(&a, &b).unwrap()).abs() < 1e-9);

        // One position differs.
        let c = LoraVector::try_new(
            vec![RawCoordinate::Float(1.0), RawCoordinate::Float(2.5)],
            2,
            VectorCoordinateType::Float32,
        )
        .unwrap();
        assert!((hamming_distance(&a, &c).unwrap() - 1.0).abs() < 1e-9);
    }
}
