//! `vector.*` — vector similarity, distance, coordinate extraction, and norms.
//!
//! VECTOR construction is handled by `cast.to(list, VECTOR<T>(N))` /
//! `CAST(list AS VECTOR<T>(N))`.

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

fn coordinates(args: &[LoraValue]) -> LoraValue {
    let Some(LoraValue::String(target)) = args.get(1) else {
        return LoraValue::Null;
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
