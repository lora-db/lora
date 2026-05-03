//! Construction surface for [`LoraVector`]: the build-error enum, the
//! raw-coordinate input shape, the validating `try_new` constructor,
//! and the string-form coordinate parser.

use std::fmt;

use super::types::{LoraVector, VectorCoordinateType, VectorValues, MAX_VECTOR_DIMENSION};

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
