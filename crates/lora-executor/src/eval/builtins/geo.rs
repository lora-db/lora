//! `geo.*` — spatial operations.
//!
//! `geo.point(map)` constructs a POINT value from a coordinate map
//! (`{longitude, latitude}`, `{x, y}`, optionally with `crs` / `srid`
//! and `height` / `z`). `cast.to(map, POINT)` / `CAST(map AS POINT)`
//! share the same map decoder.

use lora_store::point_distance;

use super::super::errors::set_eval_error;
use super::super::point::build_point_from_map;
use crate::value::LoraValue;

pub(super) fn dispatch(op: &str, args: &[LoraValue]) -> Option<LoraValue> {
    Some(match op {
        "distance" => distance(args),
        "within_bbox" => within_bbox(args),
        "point" => point(args),
        _ => return None,
    })
}

fn point(args: &[LoraValue]) -> LoraValue {
    let Some(value) = args.first() else {
        return LoraValue::Null;
    };
    match value {
        LoraValue::Null => LoraValue::Null,
        // Already a point — pass-through so `point(p)` is idempotent.
        LoraValue::Point(p) => LoraValue::Point(p.clone()),
        LoraValue::Map(m) => match build_point_from_map(m) {
            Ok(Some(p)) => LoraValue::Point(p),
            Ok(None) => LoraValue::Null,
            Err(msg) => {
                set_eval_error(msg);
                LoraValue::Null
            }
        },
        _ => {
            set_eval_error("point() expects a coordinate map".to_string());
            LoraValue::Null
        }
    }
}

fn distance(args: &[LoraValue]) -> LoraValue {
    match (args.first(), args.get(1)) {
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
    }
}

fn within_bbox(args: &[LoraValue]) -> LoraValue {
    match (args.first(), args.get(1), args.get(2)) {
        (Some(LoraValue::Point(p)), Some(LoraValue::Point(ll)), Some(LoraValue::Point(ur))) => {
            if p.srid != ll.srid || p.srid != ur.srid {
                set_eval_error(
                    "geo.within_bbox requires the point and bbox corners to share an SRID"
                        .to_string(),
                );
                return LoraValue::Null;
            }
            // A geographic box with ll.longitude > ur.longitude crosses
            // the antimeridian.
            match lora_store::bbox_contains(p, ll, ur) {
                Some(inside) => LoraValue::Bool(inside),
                None => LoraValue::Null,
            }
        }
        _ => LoraValue::Null,
    }
}
