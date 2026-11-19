//! `vector.*` — vector similarity, distance, coordinate extraction, and norms.
//!
//! VECTOR construction is handled by `cast.to(list, VECTOR<T>(N))` /
//! `CAST(list AS VECTOR<T>(N))`, and by `vector(value, dimension, TYPE)`
//! (`vector.of`), whose dimension may be any expression.

use lora_store::{parse_string_values, LoraVector, VectorCoordinateType};

use crate::value::LoraValue;

use super::super::errors::set_eval_error;
use super::super::vector::{
    eval_vector_distance_fn, eval_vector_norm_fn, eval_vector_sim_cosine, eval_vector_sim_euclidean,
};

pub(super) fn dispatch(op: &str, args: &[LoraValue]) -> Option<LoraValue> {
    Some(match op {
        "dimension" | "dim" => dimension(args),
        "distance" => distance(args),
        "similarity" => similarity(args),
        "norm" => norm(args),
        "coordinates" => coordinates(args),
        "of" => of(args),
        _ => return None,
    })
}

fn dimension(args: &[LoraValue]) -> LoraValue {
    match args.first() {
        Some(LoraValue::Null) | None => LoraValue::Null,
        Some(LoraValue::Vector(v)) => LoraValue::Int(v.dimension as i64),
        Some(other) => {
            set_eval_error(format!(
                "vector.dimension expected VECTOR, got {}",
                crate::errors::value_kind(other)
            ));
            LoraValue::Null
        }
    }
}

fn distance(args: &[LoraValue]) -> LoraValue {
    eval_vector_distance_fn(args)
}

fn similarity(args: &[LoraValue]) -> LoraValue {
    // Two valid call shapes:
    //   vector.similarity(a, b)                 — defaults to cosine
    //   vector.similarity(a, b, 'cosine'|'euclidean')
    let metric = args
        .get(2)
        .and_then(|v| match v {
            LoraValue::String(s) => Some(s.to_ascii_lowercase()),
            _ => None,
        })
        .unwrap_or_else(|| "cosine".to_string());
    match metric.as_str() {
        "cosine" => eval_vector_sim_cosine(args),
        "euclidean" => eval_vector_sim_euclidean(args),
        _ => LoraValue::Null,
    }
}

fn norm(args: &[LoraValue]) -> LoraValue {
    eval_vector_norm_fn(args)
}

/// `vector(value, dimension, coordinateType)`: a list of numbers or a
/// string such as `'[1, 2, 3]'` as a VECTOR. Unlike the cast, which yields
/// null for a value it cannot convert, a bad argument is an error.
fn of(args: &[LoraValue]) -> LoraValue {
    let (Some(value), Some(dimension), Some(coord)) = (args.first(), args.get(1), args.get(2))
    else {
        return LoraValue::Null;
    };
    if matches!(value, LoraValue::Null)
        || matches!(dimension, LoraValue::Null)
        || matches!(coord, LoraValue::Null)
    {
        return LoraValue::Null;
    }
    let fail = |msg: String| {
        set_eval_error(msg);
        LoraValue::Null
    };
    let LoraValue::Int(dimension) = dimension else {
        return fail(format!(
            "vector() dimension must be an INTEGER, got {}",
            crate::errors::value_kind(dimension)
        ));
    };
    let LoraValue::String(coord_name) = coord else {
        return fail(format!(
            "vector() coordinate type must be a type name such as FLOAT32, got {}",
            crate::errors::value_kind(coord)
        ));
    };
    let Some(coord) = VectorCoordinateType::parse(coord_name) else {
        return fail(format!("vector() unknown coordinate type `{coord_name}`"));
    };
    let raw = match value {
        LoraValue::List(items) => match super::super::vector::coerce_list_to_raw_coords(items) {
            Ok(raw) => raw,
            Err(e) => return fail(format!("vector(): {e}")),
        },
        LoraValue::String(s) => match parse_string_values(s) {
            Ok(raw) => raw,
            Err(e) => return fail(format!("vector(): {e}")),
        },
        LoraValue::Vector(v)
            if v.coordinate_type() == coord && v.dimension as i64 == *dimension =>
        {
            return value.clone()
        }
        LoraValue::Vector(v) => v
            .values
            .as_f64_vec()
            .into_iter()
            .map(lora_store::RawCoordinate::Float)
            .collect(),
        other => {
            return fail(format!(
                "vector() expected LIST<NUMBER> or STRING, got {}",
                crate::errors::value_kind(other)
            ))
        }
    };
    match LoraVector::try_new(raw, *dimension, coord) {
        Ok(v) => LoraValue::from(v),
        Err(e) => fail(format!("vector(): {e}")),
    }
}

/// `vector.coordinates(v[, INTEGER | FLOAT])`. Without a target type the
/// list keeps the vector's own kind: integers for an integer vector,
/// floats for a float vector.
fn coordinates(args: &[LoraValue]) -> LoraValue {
    let target = match args.get(1) {
        None => match args.first() {
            Some(LoraValue::Vector(v)) => match v.coordinate_type() {
                VectorCoordinateType::Float64 | VectorCoordinateType::Float32 => "FLOAT",
                _ => "INTEGER",
            },
            _ => "FLOAT",
        }
        .to_string(),
        Some(LoraValue::String(target)) => target.clone(),
        Some(_) => return LoraValue::Null,
    };

    match target.trim().to_ascii_uppercase().as_str() {
        "INTEGER" | "INT" => integer_coordinates(args),
        "FLOAT" | "REAL" => float_coordinates(args),
        _ => LoraValue::Null,
    }
}

fn integer_coordinates(args: &[LoraValue]) -> LoraValue {
    match args.first() {
        Some(LoraValue::Null) | None => LoraValue::Null,
        Some(LoraValue::Vector(v)) => LoraValue::List(
            v.values
                .to_i64_vec()
                .into_iter()
                .map(LoraValue::Int)
                .collect(),
        ),
        Some(other) => {
            set_eval_error(format!(
                "vector.coordinates expected VECTOR, got {}",
                crate::errors::value_kind(other)
            ));
            LoraValue::Null
        }
    }
}

fn float_coordinates(args: &[LoraValue]) -> LoraValue {
    match args.first() {
        Some(LoraValue::Null) | None => LoraValue::Null,
        Some(LoraValue::Vector(v)) => LoraValue::List(
            v.values
                .as_f64_vec()
                .into_iter()
                .map(LoraValue::Float)
                .collect(),
        ),
        Some(other) => {
            set_eval_error(format!(
                "vector.coordinates expected VECTOR, got {}",
                crate::errors::value_kind(other)
            ));
            LoraValue::Null
        }
    }
}
