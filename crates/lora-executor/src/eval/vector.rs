//! Vector function helpers shared by the [`super::functions::eval_function`]
//! dispatcher.
//!
//! Each helper consumes the dispatcher's `&[LoraValue]` argument slice
//! and returns a `LoraValue`, propagating null where a `null` argument
//! reached a recognised slot, calling `set_eval_error` and returning
//! `Null` when an argument violates the function contract.

use lora_store::{
    cosine_similarity_bounded, cosine_similarity_raw, dot_product, euclidean_distance,
    euclidean_distance_squared, euclidean_norm, euclidean_similarity, hamming_distance,
    manhattan_distance, manhattan_norm, parse_string_values, LoraVector, RawCoordinate,
    VectorCoordinateType,
};

use crate::value::LoraValue;

use super::errors::set_eval_error;

fn coerce_list_to_raw_coords(items: &[LoraValue]) -> Result<Vec<RawCoordinate>, String> {
    let mut out = Vec::with_capacity(items.len());
    for item in items {
        match item {
            LoraValue::Int(i) => out.push(RawCoordinate::Int(*i)),
            LoraValue::Float(f) => {
                if !f.is_finite() {
                    return Err("vector coordinates cannot be NaN or Infinity".to_string());
                }
                out.push(RawCoordinate::Float(*f));
            }
            LoraValue::List(_) => {
                return Err("vector coordinates cannot contain nested lists".to_string());
            }
            LoraValue::Null => {
                return Err("vector coordinates cannot be null".to_string());
            }
            other => {
                return Err(format!(
                    "vector coordinates must be numeric, got `{}`",
                    crate::errors::value_kind(other)
                ));
            }
        }
    }
    Ok(out)
}

pub(super) fn eval_vector_ctor(args: &[LoraValue]) -> LoraValue {
    let value = args.first();
    let dimension = args.get(1);
    let type_arg = args.get(2);

    // null propagation on value / dimension
    if matches!(value, Some(LoraValue::Null)) || matches!(dimension, Some(LoraValue::Null)) {
        return LoraValue::Null;
    }

    let Some(type_val) = type_arg else {
        return LoraValue::Null;
    };

    let type_name = match type_val {
        LoraValue::String(s) => s.clone(),
        LoraValue::Null => {
            set_eval_error("`vector()` `coordinateType` must not be null".to_string());
            return LoraValue::Null;
        }
        other => {
            set_eval_error(format!(
                "`vector()` `coordinateType` must be a string or type literal, got `{}`",
                crate::errors::value_kind(other)
            ));
            return LoraValue::Null;
        }
    };
    let Some(coordinate_type) = VectorCoordinateType::parse(&type_name) else {
        set_eval_error(format!("unknown vector coordinate type `{type_name}`"));
        return LoraValue::Null;
    };

    let dim_i64 = match dimension {
        Some(LoraValue::Int(i)) => *i,
        Some(LoraValue::Float(f)) if f.fract() == 0.0 => *f as i64,
        Some(other) => {
            set_eval_error(format!(
                "`vector()` dimension must be INTEGER, got `{}`",
                crate::errors::value_kind(other)
            ));
            return LoraValue::Null;
        }
        None => {
            set_eval_error("`vector()` requires a dimension argument".to_string());
            return LoraValue::Null;
        }
    };

    let raw = match value {
        Some(LoraValue::List(items)) => match coerce_list_to_raw_coords(items) {
            Ok(r) => r,
            Err(e) => {
                set_eval_error(e);
                return LoraValue::Null;
            }
        },
        Some(LoraValue::String(s)) => match parse_string_values(s) {
            Ok(r) => r,
            Err(e) => {
                set_eval_error(e.to_string());
                return LoraValue::Null;
            }
        },
        Some(other) => {
            set_eval_error(format!(
                "`vector()` value must be LIST<NUMBER> or STRING, got `{}`",
                crate::errors::value_kind(other)
            ));
            return LoraValue::Null;
        }
        None => {
            set_eval_error("`vector()` requires a value argument".to_string());
            return LoraValue::Null;
        }
    };

    match LoraVector::try_new(raw, dim_i64, coordinate_type) {
        Ok(v) => LoraValue::Vector(v),
        Err(e) => {
            set_eval_error(e.to_string());
            LoraValue::Null
        }
    }
}

/// Coerce either a `Vector` value or a `LIST<NUMBER>` into a `LoraVector`
/// on the fly — used by `vector.similarity.cosine` /
/// `vector.similarity.euclidean`, which accept both. A plain list is
/// converted using FLOAT32 storage with a matching dimension.
fn coerce_similarity_input(value: &LoraValue) -> Result<Option<LoraVector>, String> {
    match value {
        LoraValue::Null => Ok(None),
        LoraValue::Vector(v) => Ok(Some(v.clone())),
        LoraValue::List(items) => {
            let raw = coerce_list_to_raw_coords(items)?;
            let dim = raw.len() as i64;
            if dim == 0 {
                return Err("vector similarity cannot be computed on an empty list".to_string());
            }
            LoraVector::try_new(raw, dim, VectorCoordinateType::Float32)
                .map(Some)
                .map_err(|e| e.to_string())
        }
        other => Err(format!(
            "expected VECTOR or LIST<NUMBER>, got `{}`",
            crate::errors::value_kind(other)
        )),
    }
}

pub(super) fn eval_vector_sim_cosine(args: &[LoraValue]) -> LoraValue {
    let a = args.first();
    let b = args.get(1);
    let (Some(a), Some(b)) = (a, b) else {
        return LoraValue::Null;
    };

    let av = match coerce_similarity_input(a) {
        Ok(Some(v)) => v,
        Ok(None) => return LoraValue::Null,
        Err(e) => {
            set_eval_error(e);
            return LoraValue::Null;
        }
    };
    let bv = match coerce_similarity_input(b) {
        Ok(Some(v)) => v,
        Ok(None) => return LoraValue::Null,
        Err(e) => {
            set_eval_error(e);
            return LoraValue::Null;
        }
    };

    if av.dimension != bv.dimension {
        set_eval_error(format!(
            "`vector.similarity.cosine` requires equal dimensions, got {} and {}",
            av.dimension, bv.dimension
        ));
        return LoraValue::Null;
    }

    match cosine_similarity_bounded(&av, &bv) {
        Some(s) => LoraValue::Float(s),
        None => LoraValue::Null,
    }
}

pub(super) fn eval_vector_sim_euclidean(args: &[LoraValue]) -> LoraValue {
    let a = args.first();
    let b = args.get(1);
    let (Some(a), Some(b)) = (a, b) else {
        return LoraValue::Null;
    };

    let av = match coerce_similarity_input(a) {
        Ok(Some(v)) => v,
        Ok(None) => return LoraValue::Null,
        Err(e) => {
            set_eval_error(e);
            return LoraValue::Null;
        }
    };
    let bv = match coerce_similarity_input(b) {
        Ok(Some(v)) => v,
        Ok(None) => return LoraValue::Null,
        Err(e) => {
            set_eval_error(e);
            return LoraValue::Null;
        }
    };

    if av.dimension != bv.dimension {
        set_eval_error(format!(
            "`vector.similarity.euclidean` requires equal dimensions, got {} and {}",
            av.dimension, bv.dimension
        ));
        return LoraValue::Null;
    }

    match euclidean_similarity(&av, &bv) {
        Some(s) => LoraValue::Float(s),
        None => LoraValue::Null,
    }
}

pub(super) fn eval_vector_distance_fn(args: &[LoraValue]) -> LoraValue {
    let a = args.first();
    let b = args.get(1);
    let metric = args.get(2);

    if matches!(a, Some(LoraValue::Null)) || matches!(b, Some(LoraValue::Null)) {
        return LoraValue::Null;
    }

    let (Some(LoraValue::Vector(av)), Some(LoraValue::Vector(bv))) = (a, b) else {
        set_eval_error("`vector_distance()` requires two VECTOR arguments".to_string());
        return LoraValue::Null;
    };

    if av.dimension != bv.dimension {
        set_eval_error(format!(
            "`vector_distance()` requires equal dimensions, got {} and {}",
            av.dimension, bv.dimension
        ));
        return LoraValue::Null;
    }

    let metric_str = match metric {
        Some(LoraValue::String(s)) => s.clone(),
        Some(LoraValue::Null) => return LoraValue::Null,
        _ => {
            set_eval_error("`vector_distance()` metric must be a string/identifier".to_string());
            return LoraValue::Null;
        }
    };

    let result = match metric_str.to_ascii_uppercase().as_str() {
        "EUCLIDEAN" => euclidean_distance(av, bv),
        "EUCLIDEAN_SQUARED" => euclidean_distance_squared(av, bv),
        "MANHATTAN" => manhattan_distance(av, bv),
        "COSINE" => cosine_similarity_raw(av, bv).map(|s| 1.0 - s),
        "DOT" => dot_product(av, bv).map(|d| -d),
        "HAMMING" => hamming_distance(av, bv),
        other => {
            set_eval_error(format!("unknown vector distance metric `{other}`"));
            return LoraValue::Null;
        }
    };
    result.map(LoraValue::Float).unwrap_or(LoraValue::Null)
}

pub(super) fn eval_vector_norm_fn(args: &[LoraValue]) -> LoraValue {
    let v = args.first();
    let metric = args.get(1);

    if matches!(v, Some(LoraValue::Null)) {
        return LoraValue::Null;
    }
    let Some(LoraValue::Vector(v)) = v else {
        set_eval_error("`vector_norm()` requires a VECTOR argument".to_string());
        return LoraValue::Null;
    };

    let metric_str = match metric {
        Some(LoraValue::String(s)) => s.clone(),
        Some(LoraValue::Null) => return LoraValue::Null,
        _ => {
            set_eval_error("`vector_norm()` metric must be a string/identifier".to_string());
            return LoraValue::Null;
        }
    };

    match metric_str.to_ascii_uppercase().as_str() {
        "EUCLIDEAN" => LoraValue::Float(euclidean_norm(v)),
        "MANHATTAN" => LoraValue::Float(manhattan_norm(v)),
        other => {
            set_eval_error(format!("unknown vector norm metric `{other}`"));
            LoraValue::Null
        }
    }
}
