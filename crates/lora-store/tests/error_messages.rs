//! Regression baseline for `VectorBuildError` and `SnapshotError`
//! `Display` output.
//!
//! These messages flow into `LoraError::message()` via the executor
//! (vector errors) and the snapshot subsystem (payload errors).
//! Pinning each variant catches wording drift before it changes
//! user-visible behaviour.

use lora_store::{SnapshotError, VectorBuildError, VectorCoordinateType, MAX_VECTOR_DIMENSION};

#[test]
fn invalid_dimension() {
    let err = VectorBuildError::InvalidDimension(0);
    assert_eq!(
        err.to_string(),
        format!("vector dimension must be between 1 and {MAX_VECTOR_DIMENSION}, got 0")
    );
}

#[test]
fn dimension_mismatch() {
    let err = VectorBuildError::DimensionMismatch {
        expected: 3,
        got: 4,
    };
    assert_eq!(
        err.to_string(),
        "vector value length 4 does not match declared dimension 3"
    );
}

#[test]
fn nested_list_not_allowed() {
    assert_eq!(
        VectorBuildError::NestedListNotAllowed.to_string(),
        "vector coordinates cannot contain nested lists"
    );
}

#[test]
fn non_numeric_coordinate() {
    let err = VectorBuildError::NonNumericCoordinate("string".into());
    assert_eq!(
        err.to_string(),
        "vector coordinates must be numeric, got `string`"
    );
}

#[test]
fn non_finite_coordinate() {
    assert_eq!(
        VectorBuildError::NonFiniteCoordinate.to_string(),
        "vector coordinates cannot be NaN or Infinity"
    );
}

#[test]
fn out_of_range() {
    let err = VectorBuildError::OutOfRange {
        coordinate_type: VectorCoordinateType::Integer8,
        value: "999".into(),
    };
    assert_eq!(
        err.to_string(),
        "value `999` is out of range for coordinate type `INTEGER8`"
    );
}

#[test]
fn unknown_coordinate_type() {
    let err = VectorBuildError::UnknownCoordinateType("DOUBLE".into());
    assert_eq!(err.to_string(), "unknown vector coordinate type `DOUBLE`");
}

#[test]
fn snapshot_io() {
    let inner = std::io::Error::new(std::io::ErrorKind::NotFound, "missing");
    let err = SnapshotError::Io(inner);
    assert_eq!(err.to_string(), "snapshot I/O error: missing");
}

#[test]
fn snapshot_decode() {
    let err = SnapshotError::Decode("bad payload".into());
    assert_eq!(
        err.to_string(),
        "snapshot payload could not be decoded: bad payload"
    );
}

#[test]
fn snapshot_encode() {
    let err = SnapshotError::Encode("write failed".into());
    assert_eq!(
        err.to_string(),
        "snapshot payload could not be encoded: write failed"
    );
}
