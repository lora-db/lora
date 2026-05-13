//! Vector math: similarity, distance, and norm functions.
//!
//! All operations are dimension-checked at the boundary and use `f32`
//! arithmetic internally for parity with LoraDB's documented vector
//! function semantics; the result is widened back to `f64` for the
//! `LoraValue::Float` return path.

use super::types::LoraVector;

/// Return Some(value) if both vectors have the same dimension; None if
/// they don't. Callers route the None branch to a query error so that
/// `vector_distance` / `vector.similarity.*` never silently return a
/// bogus number.
fn check_same_dim(a: &LoraVector, b: &LoraVector) -> Option<usize> {
    if a.dimension == b.dimension && a.values.len() == a.dimension && b.values.len() == b.dimension
    {
        Some(a.dimension)
    } else {
        None
    }
}

fn for_each_pair_as_f32(a: &LoraVector, b: &LoraVector, mut f: impl FnMut(f32, f32)) -> Option<()> {
    let dim = check_same_dim(a, b)?;
    for i in 0..dim {
        f(a.values.f32_at(i)?, b.values.f32_at(i)?);
    }
    Some(())
}

fn for_each_value_as_f32(v: &LoraVector, mut f: impl FnMut(f32)) -> Option<()> {
    for i in 0..v.values.len() {
        f(v.values.f32_at(i)?);
    }
    Some(())
}

/// Raw cosine similarity in the range [-1, 1]. Returns `None` when
/// either vector has zero norm, since cosine is undefined in that case.
pub fn cosine_similarity_raw(a: &LoraVector, b: &LoraVector) -> Option<f64> {
    let mut dot = 0f32;
    let mut na = 0f32;
    let mut nb = 0f32;
    for_each_pair_as_f32(a, b, |x, y| {
        dot += x * y;
        na += x * x;
        nb += y * y;
    })?;
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
/// `vector.similarity(..., "cosine")` behaviour.
pub fn cosine_similarity_bounded(a: &LoraVector, b: &LoraVector) -> Option<f64> {
    cosine_similarity_raw(a, b).map(|raw| ((raw + 1.0) / 2.0).clamp(0.0, 1.0))
}

/// Squared Euclidean distance (sum of squared differences). Uses f32
/// arithmetic to match LoraDB's vector function implementation.
pub fn euclidean_distance_squared(a: &LoraVector, b: &LoraVector) -> Option<f64> {
    let mut sum = 0f32;
    for_each_pair_as_f32(a, b, |x, y| {
        let d = x - y;
        sum += d * d;
    })?;
    Some(sum as f64)
}

/// Euclidean (L2) distance.
pub fn euclidean_distance(a: &LoraVector, b: &LoraVector) -> Option<f64> {
    euclidean_distance_squared(a, b).map(f64::sqrt)
}

/// Manhattan (L1) distance.
pub fn manhattan_distance(a: &LoraVector, b: &LoraVector) -> Option<f64> {
    let mut sum = 0f32;
    for_each_pair_as_f32(a, b, |x, y| {
        sum += (x - y).abs();
    })?;
    Some(sum as f64)
}

/// Hamming distance: count of positions where the two vectors differ.
pub fn hamming_distance(a: &LoraVector, b: &LoraVector) -> Option<f64> {
    let mut count = 0i64;
    for_each_pair_as_f32(a, b, |x, y| {
        if x != y {
            count += 1;
        }
    })?;
    Some(count as f64)
}

/// Dot product (f32 arithmetic, widened back to f64).
pub fn dot_product(a: &LoraVector, b: &LoraVector) -> Option<f64> {
    let mut acc = 0f32;
    for_each_pair_as_f32(a, b, |x, y| {
        acc += x * y;
    })?;
    Some(acc as f64)
}

/// Euclidean (L2) norm.
pub fn euclidean_norm(v: &LoraVector) -> f64 {
    let mut sum = 0f32;
    let _ = for_each_value_as_f32(v, |x| {
        sum += x * x;
    });
    (sum.sqrt()) as f64
}

/// Manhattan (L1) norm.
pub fn manhattan_norm(v: &LoraVector) -> f64 {
    let mut sum = 0f32;
    let _ = for_each_value_as_f32(v, |x| {
        sum += x.abs();
    });
    sum as f64
}

/// Similarity score derived from squared Euclidean distance: `1 / (1 +
/// d²)`. For the documented example where `distance² == 22`, this
/// yields `1 / 23 ≈ 0.043478`.
pub fn euclidean_similarity(a: &LoraVector, b: &LoraVector) -> Option<f64> {
    euclidean_distance_squared(a, b).map(|d2| 1.0 / (1.0 + d2))
}
