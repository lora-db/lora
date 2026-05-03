//! Built-in function dispatcher.
//!
//! [`eval_function`] is the central `match` over function names that
//! the [`super::expr::eval_expr`] dispatcher hands off `Function`-shaped
//! expressions to. The body groups functions by namespace: scalar
//! identity / type interrogation, list / string manipulation, math,
//! temporal constructors, spatial functions, and the vector
//! constructor / similarity / norm helpers.
//!
//! The `point()` and `datetime()` map decoders live in `super::point`,
//! and the vector helpers live in `super::vector`.

use lora_store::{
    point_distance, GraphStorage, LoraDate, LoraDateTime, LoraDuration, LoraLocalDateTime,
    LoraLocalTime, LoraTime,
};

use crate::value::LoraValue;

use super::binops::substring_by_chars;
use super::errors::set_eval_error;
use super::expr::EvalContext;
use super::point::{build_point_from_map, timezone_name_to_offset};
use super::vector::{
    eval_vector_ctor, eval_vector_distance_fn, eval_vector_norm_fn, eval_vector_sim_cosine,
    eval_vector_sim_euclidean,
};

pub(super) fn eval_function<S: GraphStorage>(
    name: &str,
    args: &[LoraValue],
    ctx: &EvalContext<'_, S>,
) -> LoraValue {
    let fq = name.to_ascii_lowercase();

    match fq.as_str() {
        "id" => {
            if let Some(LoraValue::Node(id)) = args.first() {
                LoraValue::Int(*id as i64)
            } else if let Some(LoraValue::Relationship(id)) = args.first() {
                LoraValue::Int(*id as i64)
            } else {
                LoraValue::Null
            }
        }

        "tolower" => match args.first() {
            Some(LoraValue::String(s)) => LoraValue::String(s.to_ascii_lowercase()),
            _ => LoraValue::Null,
        },

        "toupper" => match args.first() {
            Some(LoraValue::String(s)) => LoraValue::String(s.to_ascii_uppercase()),
            _ => LoraValue::Null,
        },

        "coalesce" => {
            for arg in args {
                if !matches!(arg, LoraValue::Null) {
                    return arg.clone();
                }
            }
            LoraValue::Null
        }

        "type" => match args.first() {
            Some(LoraValue::Relationship(id)) => ctx
                .storage
                .with_relationship(*id, |r| LoraValue::String(r.rel_type.clone()))
                .unwrap_or(LoraValue::Null),
            _ => LoraValue::Null,
        },

        "labels" => match args.first() {
            Some(LoraValue::Node(id)) => ctx
                .storage
                .with_node(*id, |n| {
                    LoraValue::List(
                        n.labels
                            .iter()
                            .map(|s| LoraValue::String(s.clone()))
                            .collect(),
                    )
                })
                .unwrap_or(LoraValue::Null),
            _ => LoraValue::Null,
        },

        "keys" => match args.first() {
            Some(LoraValue::Node(id)) => ctx
                .storage
                .with_node(*id, |n| {
                    LoraValue::List(
                        n.properties
                            .keys()
                            .map(|k| LoraValue::String(k.clone()))
                            .collect(),
                    )
                })
                .unwrap_or(LoraValue::Null),
            Some(LoraValue::Relationship(id)) => ctx
                .storage
                .with_relationship(*id, |r| {
                    LoraValue::List(
                        r.properties
                            .keys()
                            .map(|k| LoraValue::String(k.clone()))
                            .collect(),
                    )
                })
                .unwrap_or(LoraValue::Null),
            Some(LoraValue::Map(m)) => {
                LoraValue::List(m.keys().cloned().map(LoraValue::String).collect())
            }
            _ => LoraValue::Null,
        },

        "size" | "length" => match args.first() {
            Some(LoraValue::List(l)) => LoraValue::Int(l.len() as i64),
            Some(LoraValue::String(s)) => LoraValue::Int(s.len() as i64),
            Some(LoraValue::Path(p)) => LoraValue::Int(p.rels.len() as i64),
            Some(LoraValue::Vector(v)) => LoraValue::Int(v.dimension as i64),
            _ => LoraValue::Null,
        },

        "nodes" => match args.first() {
            Some(LoraValue::Path(p)) => {
                LoraValue::List(p.nodes.iter().map(|id| LoraValue::Node(*id)).collect())
            }
            _ => LoraValue::Null,
        },

        "relationships" => match args.first() {
            Some(LoraValue::Path(p)) => LoraValue::List(
                p.rels
                    .iter()
                    .map(|id| LoraValue::Relationship(*id))
                    .collect(),
            ),
            _ => LoraValue::Null,
        },

        "head" => match args.first() {
            Some(LoraValue::List(l)) => l.first().cloned().unwrap_or(LoraValue::Null),
            _ => LoraValue::Null,
        },

        "tail" => match args.first() {
            Some(LoraValue::List(l)) => {
                if l.is_empty() {
                    LoraValue::Null
                } else {
                    LoraValue::List(l[1..].to_vec())
                }
            }
            _ => LoraValue::Null,
        },

        "tostring" => match args.first() {
            Some(LoraValue::Int(i)) => LoraValue::String(i.to_string()),
            Some(LoraValue::Float(f)) => LoraValue::String(f.to_string()),
            Some(LoraValue::Bool(b)) => LoraValue::String(b.to_string()),
            Some(LoraValue::String(s)) => LoraValue::String(s.clone()),
            Some(LoraValue::Null) => LoraValue::Null,
            Some(LoraValue::Date(d)) => LoraValue::String(d.to_string()),
            Some(LoraValue::DateTime(dt)) => LoraValue::String(dt.to_string()),
            Some(LoraValue::LocalDateTime(dt)) => LoraValue::String(dt.to_string()),
            Some(LoraValue::Time(t)) => LoraValue::String(t.to_string()),
            Some(LoraValue::LocalTime(t)) => LoraValue::String(t.to_string()),
            Some(LoraValue::Duration(dur)) => LoraValue::String(dur.to_string()),
            _ => LoraValue::Null,
        },

        "tointeger" | "toint" => match args.first() {
            Some(LoraValue::Int(i)) => LoraValue::Int(*i),
            Some(LoraValue::Float(f)) => LoraValue::Int(*f as i64),
            Some(LoraValue::String(s)) => s
                .parse::<i64>()
                .ok()
                .map(LoraValue::Int)
                .unwrap_or(LoraValue::Null),
            _ => LoraValue::Null,
        },

        "tofloat" => match args.first() {
            Some(LoraValue::Float(f)) => LoraValue::Float(*f),
            Some(LoraValue::Int(i)) => LoraValue::Float(*i as f64),
            Some(LoraValue::String(s)) => s
                .parse::<f64>()
                .ok()
                .map(LoraValue::Float)
                .unwrap_or(LoraValue::Null),
            _ => LoraValue::Null,
        },

        "abs" => match args.first() {
            Some(LoraValue::Int(i)) => LoraValue::Int(i.abs()),
            Some(LoraValue::Float(f)) => LoraValue::Float(f.abs()),
            _ => LoraValue::Null,
        },

        // -- Math functions ------------------------------------------------
        "ceil" => match args.first() {
            Some(LoraValue::Float(f)) => LoraValue::Int(f.ceil() as i64),
            Some(LoraValue::Int(i)) => LoraValue::Int(*i),
            _ => LoraValue::Null,
        },

        "floor" => match args.first() {
            Some(LoraValue::Float(f)) => LoraValue::Int(f.floor() as i64),
            Some(LoraValue::Int(i)) => LoraValue::Int(*i),
            _ => LoraValue::Null,
        },

        "round" => match args.first() {
            Some(LoraValue::Float(f)) => LoraValue::Int(f.round() as i64),
            Some(LoraValue::Int(i)) => LoraValue::Int(*i),
            _ => LoraValue::Null,
        },

        "sqrt" => match args.first() {
            Some(LoraValue::Float(f)) => {
                if *f < 0.0 {
                    LoraValue::Null
                } else {
                    LoraValue::Float(f.sqrt())
                }
            }
            Some(LoraValue::Int(i)) => {
                if *i < 0 {
                    LoraValue::Null
                } else {
                    LoraValue::Float((*i as f64).sqrt())
                }
            }
            _ => LoraValue::Null,
        },

        "sign" => match args.first() {
            Some(LoraValue::Int(i)) => LoraValue::Int(i.signum()),
            Some(LoraValue::Float(f)) => {
                if f.is_nan() {
                    LoraValue::Null
                } else {
                    LoraValue::Int(f.signum() as i64)
                }
            }
            _ => LoraValue::Null,
        },

        // -- String functions -----------------------------------------------
        "trim" => match args.first() {
            Some(LoraValue::String(s)) => LoraValue::String(s.trim().to_string()),
            _ => LoraValue::Null,
        },

        "ltrim" => match args.first() {
            Some(LoraValue::String(s)) => LoraValue::String(s.trim_start().to_string()),
            _ => LoraValue::Null,
        },

        "rtrim" => match args.first() {
            Some(LoraValue::String(s)) => LoraValue::String(s.trim_end().to_string()),
            _ => LoraValue::Null,
        },

        "replace" => match (args.first(), args.get(1), args.get(2)) {
            (
                Some(LoraValue::String(s)),
                Some(LoraValue::String(search)),
                Some(LoraValue::String(replacement)),
            ) => LoraValue::String(s.replace(search.as_str(), replacement.as_str())),
            _ => LoraValue::Null,
        },

        "split" => match (args.first(), args.get(1)) {
            (Some(LoraValue::String(s)), Some(LoraValue::String(delimiter))) => LoraValue::List(
                s.split(delimiter.as_str())
                    .map(|part| LoraValue::String(part.to_string()))
                    .collect(),
            ),
            _ => LoraValue::Null,
        },

        "substring" => match args.first() {
            Some(LoraValue::String(s)) => {
                let start = args.get(1).and_then(|v| v.as_i64()).unwrap_or(0);
                let length = args.get(2).and_then(|v| v.as_i64());
                LoraValue::String(substring_by_chars(s, start, length))
            }
            _ => LoraValue::Null,
        },

        "reverse" => match args.first() {
            Some(LoraValue::String(s)) => LoraValue::String(s.chars().rev().collect()),
            Some(LoraValue::List(l)) => LoraValue::List(l.iter().rev().cloned().collect()),
            _ => LoraValue::Null,
        },

        "left" => match (args.first(), args.get(1)) {
            (Some(LoraValue::String(s)), Some(LoraValue::Int(n))) => {
                let n = (*n).max(0) as usize;
                LoraValue::String(s.chars().take(n).collect())
            }
            _ => LoraValue::Null,
        },

        "right" => match (args.first(), args.get(1)) {
            (Some(LoraValue::String(s)), Some(LoraValue::Int(n))) => {
                let n = (*n).max(0) as usize;
                let char_count = s.chars().count();
                let skip = char_count.saturating_sub(n);
                LoraValue::String(s.chars().skip(skip).collect())
            }
            _ => LoraValue::Null,
        },

        "properties" => match args.first() {
            Some(LoraValue::Node(id)) => ctx
                .storage
                .with_node(*id, |n| {
                    LoraValue::Map(
                        n.properties
                            .iter()
                            .map(|(k, v)| (k.clone(), LoraValue::from(v)))
                            .collect(),
                    )
                })
                .unwrap_or(LoraValue::Null),
            Some(LoraValue::Relationship(id)) => ctx
                .storage
                .with_relationship(*id, |r| {
                    LoraValue::Map(
                        r.properties
                            .iter()
                            .map(|(k, v)| (k.clone(), LoraValue::from(v)))
                            .collect(),
                    )
                })
                .unwrap_or(LoraValue::Null),
            Some(LoraValue::Map(m)) => LoraValue::Map(m.clone()),
            _ => LoraValue::Null,
        },

        "timestamp" => {
            use std::time::{SystemTime, UNIX_EPOCH};
            let millis = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|d| d.as_millis() as i64)
                .unwrap_or(0);
            LoraValue::Int(millis)
        }

        "range" => {
            let start = args.first().and_then(|v| v.as_i64()).unwrap_or(0);
            let end = args.get(1).and_then(|v| v.as_i64()).unwrap_or(0);
            let step = args.get(2).and_then(|v| v.as_i64()).unwrap_or(1);
            if step == 0 {
                return LoraValue::Null;
            }
            let mut result = Vec::new();
            let mut i = start;
            if step > 0 {
                while i <= end {
                    result.push(LoraValue::Int(i));
                    i += step;
                }
            } else {
                while i >= end {
                    result.push(LoraValue::Int(i));
                    i += step;
                }
            }
            LoraValue::List(result)
        }

        // -- Last (list) -----------------------------------------------------
        "last" => match args.first() {
            Some(LoraValue::List(l)) => l.last().cloned().unwrap_or(LoraValue::Null),
            _ => LoraValue::Null,
        },

        // -- String padding / char_length / normalize -------------------------
        "lpad" => match (args.first(), args.get(1), args.get(2)) {
            (Some(LoraValue::String(s)), Some(len_val), Some(LoraValue::String(pad))) => {
                let target_len = len_val.as_i64().unwrap_or(0).max(0) as usize;
                let current_len = s.chars().count();
                if current_len >= target_len {
                    LoraValue::String(s.clone())
                } else {
                    let pad_needed = target_len - current_len;
                    let pad_chars: String = pad.chars().cycle().take(pad_needed).collect();
                    LoraValue::String(format!("{}{}", pad_chars, s))
                }
            }
            _ => LoraValue::Null,
        },

        "rpad" => match (args.first(), args.get(1), args.get(2)) {
            (Some(LoraValue::String(s)), Some(len_val), Some(LoraValue::String(pad))) => {
                let target_len = len_val.as_i64().unwrap_or(0).max(0) as usize;
                let current_len = s.chars().count();
                if current_len >= target_len {
                    LoraValue::String(s.clone())
                } else {
                    let pad_needed = target_len - current_len;
                    let pad_chars: String = pad.chars().cycle().take(pad_needed).collect();
                    LoraValue::String(format!("{}{}", s, pad_chars))
                }
            }
            _ => LoraValue::Null,
        },

        "char_length" => match args.first() {
            Some(LoraValue::String(s)) => LoraValue::Int(s.chars().count() as i64),
            _ => LoraValue::Null,
        },

        "normalize" => match args.first() {
            // Basic NFC normalization — for ASCII input, returns as-is
            Some(LoraValue::String(s)) => LoraValue::String(s.clone()),
            _ => LoraValue::Null,
        },

        // -- toBoolean --------------------------------------------------------
        "toboolean" | "tobooleanornull" => match args.first() {
            Some(LoraValue::Bool(b)) => LoraValue::Bool(*b),
            Some(LoraValue::String(s)) => match s.to_ascii_lowercase().as_str() {
                "true" => LoraValue::Bool(true),
                "false" => LoraValue::Bool(false),
                _ => LoraValue::Null,
            },
            Some(LoraValue::Int(i)) => match *i {
                0 => LoraValue::Bool(false),
                _ => LoraValue::Bool(true),
            },
            Some(LoraValue::Null) => LoraValue::Null,
            _ => LoraValue::Null,
        },

        // -- valueType --------------------------------------------------------
        "valuetype" => match args.first() {
            Some(LoraValue::Null) => LoraValue::String("NULL".to_string()),
            Some(LoraValue::Bool(_)) => LoraValue::String("BOOLEAN".to_string()),
            Some(LoraValue::Int(_)) => LoraValue::String("INTEGER".to_string()),
            Some(LoraValue::Float(_)) => LoraValue::String("FLOAT".to_string()),
            Some(LoraValue::String(_)) => LoraValue::String("STRING".to_string()),
            Some(LoraValue::Binary(_)) => LoraValue::String("BINARY".to_string()),
            Some(LoraValue::List(items)) => {
                // Determine element type for homogeneous lists
                let elem_type = if items.is_empty() {
                    "ANY"
                } else {
                    let first_type = match &items[0] {
                        LoraValue::Int(_) => "INTEGER",
                        LoraValue::Float(_) => "FLOAT",
                        LoraValue::String(_) => "STRING",
                        LoraValue::Bool(_) => "BOOLEAN",
                        LoraValue::Null => "ANY",
                        _ => "ANY",
                    };
                    let homogeneous = items.iter().all(|v| {
                        matches!(
                            (v, first_type),
                            (LoraValue::Int(_), "INTEGER")
                                | (LoraValue::Float(_), "FLOAT")
                                | (LoraValue::String(_), "STRING")
                                | (LoraValue::Bool(_), "BOOLEAN")
                        )
                    });
                    if homogeneous {
                        first_type
                    } else {
                        "ANY"
                    }
                };
                LoraValue::String(format!("LIST<{elem_type}>"))
            }
            Some(LoraValue::Map(_)) => LoraValue::String("MAP".to_string()),
            Some(LoraValue::Node(_)) => LoraValue::String("NODE".to_string()),
            Some(LoraValue::Relationship(_)) => LoraValue::String("RELATIONSHIP".to_string()),
            Some(LoraValue::Path(_)) => LoraValue::String("PATH".to_string()),
            Some(LoraValue::Date(_)) => LoraValue::String("DATE".to_string()),
            Some(LoraValue::DateTime(_)) => LoraValue::String("DATE_TIME".to_string()),
            Some(LoraValue::LocalDateTime(_)) => LoraValue::String("LOCAL_DATE_TIME".to_string()),
            Some(LoraValue::Time(_)) => LoraValue::String("TIME".to_string()),
            Some(LoraValue::LocalTime(_)) => LoraValue::String("LOCAL_TIME".to_string()),
            Some(LoraValue::Duration(_)) => LoraValue::String("DURATION".to_string()),
            Some(LoraValue::Point(_)) => LoraValue::String("POINT".to_string()),
            Some(LoraValue::Vector(v)) => LoraValue::String(format!(
                "VECTOR<{}>({})",
                v.coordinate_type().as_str(),
                v.dimension
            )),
            None => LoraValue::Null,
        },

        // -- Trigonometric / logarithmic / constants --------------------------
        "log" | "ln" => match args.first() {
            Some(v) => match v.as_f64() {
                Some(f) if f > 0.0 => LoraValue::Float(f.ln()),
                _ => LoraValue::Null,
            },
            _ => LoraValue::Null,
        },

        "log10" => match args.first() {
            Some(v) => match v.as_f64() {
                Some(f) if f > 0.0 => LoraValue::Float(f.log10()),
                _ => LoraValue::Null,
            },
            _ => LoraValue::Null,
        },

        "exp" => match args.first() {
            Some(v) => match v.as_f64() {
                Some(f) => LoraValue::Float(f.exp()),
                None => LoraValue::Null,
            },
            _ => LoraValue::Null,
        },

        "sin" => match args.first() {
            Some(v) => match v.as_f64() {
                Some(f) => LoraValue::Float(f.sin()),
                None => LoraValue::Null,
            },
            _ => LoraValue::Null,
        },

        "cos" => match args.first() {
            Some(v) => match v.as_f64() {
                Some(f) => LoraValue::Float(f.cos()),
                None => LoraValue::Null,
            },
            _ => LoraValue::Null,
        },

        "tan" => match args.first() {
            Some(v) => match v.as_f64() {
                Some(f) => LoraValue::Float(f.tan()),
                None => LoraValue::Null,
            },
            _ => LoraValue::Null,
        },

        "asin" => match args.first() {
            Some(v) => match v.as_f64() {
                Some(f) if (-1.0..=1.0).contains(&f) => LoraValue::Float(f.asin()),
                _ => LoraValue::Null,
            },
            _ => LoraValue::Null,
        },

        "acos" => match args.first() {
            Some(v) => match v.as_f64() {
                Some(f) if (-1.0..=1.0).contains(&f) => LoraValue::Float(f.acos()),
                _ => LoraValue::Null,
            },
            _ => LoraValue::Null,
        },

        "atan" => match args.first() {
            Some(v) => match v.as_f64() {
                Some(f) => LoraValue::Float(f.atan()),
                None => LoraValue::Null,
            },
            _ => LoraValue::Null,
        },

        "atan2" => match (args.first(), args.get(1)) {
            (Some(y_val), Some(x_val)) => match (y_val.as_f64(), x_val.as_f64()) {
                (Some(y), Some(x)) => LoraValue::Float(y.atan2(x)),
                _ => LoraValue::Null,
            },
            _ => LoraValue::Null,
        },

        "pi" => LoraValue::Float(std::f64::consts::PI),

        "e" => LoraValue::Float(std::f64::consts::E),

        "rand" => {
            use std::time::{SystemTime, UNIX_EPOCH};
            // Simple pseudo-random using system time nanoseconds
            let nanos = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|d| d.subsec_nanos())
                .unwrap_or(0);
            // Use a simple hash to get pseudo-random distribution
            let hash = ((nanos as u64)
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407)) as f64;
            LoraValue::Float((hash / u64::MAX as f64).abs())
        }

        "degrees" => match args.first() {
            Some(v) => match v.as_f64() {
                Some(f) => LoraValue::Float(f.to_degrees()),
                None => LoraValue::Null,
            },
            _ => LoraValue::Null,
        },

        "radians" => match args.first() {
            Some(v) => match v.as_f64() {
                Some(f) => LoraValue::Float(f.to_radians()),
                None => LoraValue::Null,
            },
            _ => LoraValue::Null,
        },

        // -- Temporal constructors -------------------------------------------
        "date" => match args.first() {
            None => LoraValue::Date(LoraDate::today()),
            Some(LoraValue::String(s)) => match LoraDate::parse(s) {
                Ok(d) => LoraValue::Date(d),
                Err(e) => {
                    set_eval_error(e);
                    LoraValue::Null
                }
            },
            Some(LoraValue::Map(m)) => {
                let year = m.get("year").and_then(|v| v.as_i64()).unwrap_or(0) as i32;
                let month = m.get("month").and_then(|v| v.as_i64()).unwrap_or(1) as u32;
                let day = m.get("day").and_then(|v| v.as_i64()).unwrap_or(1) as u32;
                match LoraDate::new(year, month, day) {
                    Ok(d) => LoraValue::Date(d),
                    Err(e) => {
                        set_eval_error(e);
                        LoraValue::Null
                    }
                }
            }
            // Roundtrip: date(date) -> date
            Some(LoraValue::Date(d)) => LoraValue::Date(d.clone()),
            _ => LoraValue::Null,
        },

        "datetime" => match args.first() {
            None => LoraValue::DateTime(LoraDateTime::now()),
            Some(LoraValue::String(s)) => match LoraDateTime::parse(s) {
                Ok(dt) => LoraValue::DateTime(dt),
                Err(e) => {
                    set_eval_error(e);
                    LoraValue::Null
                }
            },
            Some(LoraValue::Map(m)) => {
                let year = m.get("year").and_then(|v| v.as_i64()).unwrap_or(0) as i32;
                let month = m.get("month").and_then(|v| v.as_i64()).unwrap_or(1) as u32;
                let day = m.get("day").and_then(|v| v.as_i64()).unwrap_or(1) as u32;
                let hour = m.get("hour").and_then(|v| v.as_i64()).unwrap_or(0) as u32;
                let minute = m.get("minute").and_then(|v| v.as_i64()).unwrap_or(0) as u32;
                let second = m.get("second").and_then(|v| v.as_i64()).unwrap_or(0) as u32;
                let ms = m.get("millisecond").and_then(|v| v.as_i64()).unwrap_or(0) as u32;
                let offset = if let Some(LoraValue::String(tz)) = m.get("timezone") {
                    // Simple named timezone handling: map common names to offsets
                    timezone_name_to_offset(tz)
                } else {
                    0
                };
                match LoraDateTime::new(
                    year,
                    month,
                    day,
                    hour,
                    minute,
                    second,
                    ms * 1_000_000,
                    offset,
                ) {
                    Ok(dt) => LoraValue::DateTime(dt),
                    Err(e) => {
                        set_eval_error(e);
                        LoraValue::Null
                    }
                }
            }
            _ => LoraValue::Null,
        },

        "time" => match args.first() {
            None => LoraValue::Time(LoraTime::now()),
            Some(LoraValue::String(s)) => match LoraTime::parse(s) {
                Ok(t) => LoraValue::Time(t),
                Err(e) => {
                    set_eval_error(e);
                    LoraValue::Null
                }
            },
            _ => LoraValue::Null,
        },

        "localtime" => match args.first() {
            None => LoraValue::LocalTime(LoraLocalTime::now()),
            Some(LoraValue::String(s)) => match LoraLocalTime::parse(s) {
                Ok(t) => LoraValue::LocalTime(t),
                Err(e) => {
                    set_eval_error(e);
                    LoraValue::Null
                }
            },
            _ => LoraValue::Null,
        },

        "localdatetime" => match args.first() {
            None => LoraValue::LocalDateTime(LoraLocalDateTime::now()),
            Some(LoraValue::String(s)) => match LoraLocalDateTime::parse(s) {
                Ok(dt) => LoraValue::LocalDateTime(dt),
                Err(e) => {
                    set_eval_error(e);
                    LoraValue::Null
                }
            },
            _ => LoraValue::Null,
        },

        "duration" => match args.first() {
            Some(LoraValue::String(s)) => match LoraDuration::parse(s) {
                Ok(d) => LoraValue::Duration(d),
                Err(e) => {
                    set_eval_error(e);
                    LoraValue::Null
                }
            },
            Some(LoraValue::Map(m)) => {
                let years = m.get("years").and_then(|v| v.as_i64()).unwrap_or(0);
                let months = m.get("months").and_then(|v| v.as_i64()).unwrap_or(0);
                let days = m.get("days").and_then(|v| v.as_i64()).unwrap_or(0);
                let hours = m.get("hours").and_then(|v| v.as_i64()).unwrap_or(0);
                let minutes = m.get("minutes").and_then(|v| v.as_i64()).unwrap_or(0);
                let seconds = m.get("seconds").and_then(|v| v.as_i64()).unwrap_or(0);
                LoraValue::Duration(LoraDuration {
                    months: years * 12 + months,
                    days,
                    seconds: hours * 3600 + minutes * 60 + seconds,
                    nanoseconds: 0,
                })
            }
            _ => LoraValue::Null,
        },

        // -- Temporal namespace functions -----------------------------------
        "date.truncate" => match (args.first(), args.get(1)) {
            (Some(LoraValue::String(unit)), Some(LoraValue::Date(d))) => match unit.as_str() {
                "month" => LoraValue::Date(d.truncate_to_month()),
                "year" => LoraValue::Date(LoraDate {
                    year: d.year,
                    month: 1,
                    day: 1,
                }),
                _ => LoraValue::Date(d.clone()),
            },
            _ => LoraValue::Null,
        },

        "datetime.truncate" => match (args.first(), args.get(1)) {
            (Some(LoraValue::String(unit)), Some(LoraValue::DateTime(dt))) => match unit.as_str() {
                "day" => LoraValue::DateTime(dt.truncate_to_day()),
                "hour" => LoraValue::DateTime(dt.truncate_to_hour()),
                "month" => LoraValue::DateTime(LoraDateTime {
                    year: dt.year,
                    month: dt.month,
                    day: 1,
                    hour: 0,
                    minute: 0,
                    second: 0,
                    nanosecond: 0,
                    offset_seconds: dt.offset_seconds,
                }),
                _ => LoraValue::DateTime(dt.clone()),
            },
            _ => LoraValue::Null,
        },

        "duration.between" => match (args.first(), args.get(1)) {
            (Some(LoraValue::Date(d1)), Some(LoraValue::Date(d2))) => {
                LoraValue::Duration(LoraDuration::between_dates(d1, d2))
            }
            (Some(LoraValue::DateTime(dt1)), Some(LoraValue::DateTime(dt2))) => {
                LoraValue::Duration(LoraDuration::between_datetimes(dt1, dt2))
            }
            _ => LoraValue::Null,
        },

        "duration.indays" => match (args.first(), args.get(1)) {
            (Some(LoraValue::Date(d1)), Some(LoraValue::Date(d2))) => {
                LoraValue::Duration(LoraDuration::in_days(d1, d2))
            }
            _ => LoraValue::Null,
        },

        // -- Spatial functions -----------------------------------------------
        "point" => match args.first() {
            None | Some(LoraValue::Null) => LoraValue::Null,
            Some(LoraValue::Map(m)) => match build_point_from_map(m) {
                Ok(Some(p)) => LoraValue::Point(p),
                Ok(None) => LoraValue::Null,
                Err(msg) => {
                    set_eval_error(msg);
                    LoraValue::Null
                }
            },
            Some(_) => {
                set_eval_error("point() requires a map argument".to_string());
                LoraValue::Null
            }
        },

        "distance" => match (args.first(), args.get(1)) {
            (Some(LoraValue::Point(a)), Some(LoraValue::Point(b))) => match point_distance(a, b) {
                Some(d) => LoraValue::Float(d),
                None => {
                    set_eval_error(
                        "Cannot compute distance between points with different SRIDs".to_string(),
                    );
                    LoraValue::Null
                }
            },
            _ => LoraValue::Null,
        },

        // -- Vector construction --------------------------------------------
        "vector" => eval_vector_ctor(args),

        "tointegerlist" => match args.first() {
            Some(LoraValue::Null) => LoraValue::Null,
            Some(LoraValue::Vector(v)) => LoraValue::List(
                v.values
                    .to_i64_vec()
                    .into_iter()
                    .map(LoraValue::Int)
                    .collect(),
            ),
            Some(other) => {
                set_eval_error(format!(
                    "toIntegerList() expected VECTOR, got {}",
                    crate::errors::value_kind(other)
                ));
                LoraValue::Null
            }
            None => LoraValue::Null,
        },

        "tofloatlist" => match args.first() {
            Some(LoraValue::Null) => LoraValue::Null,
            Some(LoraValue::Vector(v)) => LoraValue::List(
                v.values
                    .as_f64_vec()
                    .into_iter()
                    .map(LoraValue::Float)
                    .collect(),
            ),
            Some(other) => {
                set_eval_error(format!(
                    "toFloatList() expected VECTOR, got {}",
                    crate::errors::value_kind(other)
                ));
                LoraValue::Null
            }
            None => LoraValue::Null,
        },

        "vector_dimension_count" => match args.first() {
            Some(LoraValue::Null) => LoraValue::Null,
            Some(LoraValue::Vector(v)) => LoraValue::Int(v.dimension as i64),
            Some(other) => {
                set_eval_error(format!(
                    "vector_dimension_count() expected VECTOR, got {}",
                    crate::errors::value_kind(other)
                ));
                LoraValue::Null
            }
            None => LoraValue::Null,
        },

        "vector.similarity.cosine" => eval_vector_sim_cosine(args),
        "vector.similarity.euclidean" => eval_vector_sim_euclidean(args),
        "vector_distance" => eval_vector_distance_fn(args),
        "vector_norm" => eval_vector_norm_fn(args),

        _ => LoraValue::Null,
    }
}
