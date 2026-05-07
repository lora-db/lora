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
fn parse_coordinate_type_implements_from_str() {
    assert_eq!(
        "signed integer".parse::<VectorCoordinateType>().unwrap(),
        VectorCoordinateType::Integer64
    );
    assert!("bogus".parse::<VectorCoordinateType>().is_err());
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
