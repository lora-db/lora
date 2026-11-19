//! The `where` option of `db.index.vector.queryNodes` /
//! `queryRelationships` (taste N-01): property conditions that pick the
//! entities a nearest-neighbour search may return *before* it ranks them,
//! so the k results are the k nearest among the matches rather than the
//! matches among the k nearest.
//!
//! ```cypher
//! CALL db.index.vector.queryNodes('event_taste', 20, $me, {where: {
//!   startsAt: {gte: $from, lt: $to},
//!   location: {withinBBox: [$sw, $ne]},
//!   genre: ['techno', 'house']
//! }}) YIELD node, score
//! ```
//!
//! Each entry is `property: condition`. A condition is a value (`=`), a
//! list (`IN`), or a map of operators: `eq`, `in`, `gt`, `gte`, `lt`,
//! `lte`, `withinBBox: [lowerLeft, upperRight]`. All entries must hold. A
//! condition on an indexed property (RANGE, POINT) is answered by the index;
//! every candidate is then checked exactly, with the comparison semantics
//! of `WHERE` (a missing property never matches).

use std::collections::BTreeSet;

use lora_store::{GraphStorage, LoraPoint, StoredIndexEntity};

use crate::eval::{compare_values, value_eq};
use crate::value::{lora_value_to_property, LoraValue};

#[derive(Debug, Clone)]
enum Condition {
    /// `=` one of these values.
    OneOf(Vec<LoraValue>),
    Range {
        lo: Option<(LoraValue, bool)>,
        hi: Option<(LoraValue, bool)>,
    },
    BBox(LoraPoint, LoraPoint),
}

/// A parsed `where` option.
#[derive(Debug, Clone, Default)]
pub struct VectorFilter {
    conditions: Vec<(String, Vec<Condition>)>,
}

const OPERATORS: &str = "eq, in, gt, gte, lt, lte, withinBBox";

impl VectorFilter {
    pub fn parse(value: &LoraValue) -> Result<Self, String> {
        let LoraValue::Map(entries) = value else {
            return Err(format!(
                "`where` must be a MAP of property conditions, got {}",
                kind(value)
            ));
        };
        let mut conditions = Vec::with_capacity(entries.len());
        for (property, condition) in entries {
            conditions.push((property.clone(), parse_condition(property, condition)?));
        }
        Ok(Self { conditions })
    }

    pub fn is_empty(&self) -> bool {
        self.conditions.is_empty()
    }

    /// The ids in `scope` (the index's label or relationship type) that
    /// meet every condition.
    pub fn matching_ids<S: GraphStorage>(
        &self,
        storage: &S,
        entity: StoredIndexEntity,
        scope: &str,
    ) -> BTreeSet<u64> {
        // One index probe, for the condition likely to keep the fewest
        // entities; the others are checked per candidate. Probing every
        // indexed condition to keep the smallest set cost more than it
        // saved: a 16% date window built a 49k-id set to discard it for a
        // 1% bounding box.
        let mut ranked: Vec<(&str, &Condition)> = self
            .conditions
            .iter()
            .flat_map(|(property, conditions)| conditions.iter().map(|c| (property.as_str(), c)))
            .collect();
        ranked.sort_by_key(|(_, c)| c.selectivity_rank());
        let probed = ranked
            .into_iter()
            .find_map(|(property, c)| index_candidates(storage, entity, scope, property, c));
        let candidates = probed.unwrap_or_else(|| match entity {
            StoredIndexEntity::Node => storage.node_ids_by_label(scope),
            StoredIndexEntity::Relationship => storage.rel_ids_by_type(scope),
        });
        candidates
            .into_iter()
            .filter(|&id| self.matches(storage, entity, scope, id))
            .collect()
    }

    fn matches<S: GraphStorage>(
        &self,
        storage: &S,
        entity: StoredIndexEntity,
        scope: &str,
        id: u64,
    ) -> bool {
        let check = |get: &dyn Fn(&str) -> Option<LoraValue>| {
            self.conditions.iter().all(|(property, conditions)| {
                let Some(value) = get(property) else {
                    return false;
                };
                conditions.iter().all(|c| holds(c, &value))
            })
        };
        match entity {
            StoredIndexEntity::Node => {
                storage.node_has_label(id, scope)
                    && storage
                        .with_node(id, |node| {
                            let props = node.properties();
                            check(&|key| props.get(key).map(LoraValue::from))
                        })
                        .unwrap_or(false)
            }
            StoredIndexEntity::Relationship => storage
                .with_relationship(id, |rel| {
                    let props = rel.properties();
                    rel.rel_type() == scope && check(&|key| props.get(key).map(LoraValue::from))
                })
                .unwrap_or(false),
        }
    }
}

fn kind(value: &LoraValue) -> String {
    crate::errors::value_kind(value)
}

fn parse_condition(property: &str, value: &LoraValue) -> Result<Vec<Condition>, String> {
    let not_null = |v: &LoraValue| -> Result<LoraValue, String> {
        if matches!(v, LoraValue::Null) {
            Err(format!("`where` condition on `{property}` is null"))
        } else {
            Ok(v.clone())
        }
    };
    match value {
        LoraValue::Null => Err(format!("`where` condition on `{property}` is null")),
        LoraValue::List(items) => Ok(vec![Condition::OneOf(items.clone())]),
        LoraValue::Map(ops) => {
            let mut out = Vec::new();
            let mut lo: Option<(LoraValue, bool)> = None;
            let mut hi: Option<(LoraValue, bool)> = None;
            for (op, arg) in ops {
                match op.as_str() {
                    "eq" => out.push(Condition::OneOf(vec![not_null(arg)?])),
                    "in" => match arg {
                        LoraValue::List(items) => out.push(Condition::OneOf(items.clone())),
                        other => {
                            return Err(format!(
                                "`in` on `{property}` needs a LIST, got {}",
                                kind(other)
                            ))
                        }
                    },
                    "gt" | "gte" => {
                        if lo.is_some() {
                            return Err(format!("`{property}` has both `gt` and `gte`"));
                        }
                        lo = Some((not_null(arg)?, op == "gte"));
                    }
                    "lt" | "lte" => {
                        if hi.is_some() {
                            return Err(format!("`{property}` has both `lt` and `lte`"));
                        }
                        hi = Some((not_null(arg)?, op == "lte"));
                    }
                    "withinBBox" => match arg {
                        LoraValue::List(corners) => match corners.as_slice() {
                            [LoraValue::Point(ll), LoraValue::Point(ur)] => {
                                if ll.srid != ur.srid {
                                    return Err(format!(
                                        "`withinBBox` on `{property}`: the corners have different SRIDs"
                                    ));
                                }
                                out.push(Condition::BBox((**ll).clone(), (**ur).clone()))
                            }
                            _ => {
                                return Err(format!(
                                    "`withinBBox` on `{property}` needs [lowerLeft, upperRight] points"
                                ))
                            }
                        },
                        other => {
                            return Err(format!(
                                "`withinBBox` on `{property}` needs [lowerLeft, upperRight] points, got {}",
                                kind(other)
                            ))
                        }
                    },
                    other => {
                        return Err(format!(
                            "unknown operator `{other}` on `{property}` (known: {OPERATORS})"
                        ))
                    }
                }
            }
            if lo.is_some() || hi.is_some() {
                out.push(Condition::Range { lo, hi });
            }
            if out.is_empty() {
                return Err(format!(
                    "`where` condition on `{property}` has no operator (known: {OPERATORS})"
                ));
            }
            Ok(out)
        }
        scalar => Ok(vec![Condition::OneOf(vec![scalar.clone()])]),
    }
}

impl Condition {
    /// Lower is expected to match fewer entities: equality to a few
    /// values, then a box, then a closed range, then an open one.
    fn selectivity_rank(&self) -> u8 {
        match self {
            Condition::OneOf(values) if values.len() <= 16 => 0,
            Condition::BBox(..) => 1,
            Condition::Range {
                lo: Some(_),
                hi: Some(_),
            } => 2,
            Condition::OneOf(_) => 3,
            Condition::Range { .. } => 4,
        }
    }
}

fn holds(condition: &Condition, value: &LoraValue) -> bool {
    use std::cmp::Ordering::*;
    match condition {
        Condition::OneOf(values) => values.iter().any(|v| value_eq(value, v)),
        Condition::Range { lo, hi } => {
            let lo_ok = lo.as_ref().is_none_or(|(bound, inclusive)| {
                matches!(
                    (compare_values(value, bound), inclusive),
                    (Some(Greater), _) | (Some(Equal), true)
                )
            });
            let hi_ok = hi.as_ref().is_none_or(|(bound, inclusive)| {
                matches!(
                    (compare_values(value, bound), inclusive),
                    (Some(Less), _) | (Some(Equal), true)
                )
            });
            lo_ok && hi_ok
        }
        Condition::BBox(ll, ur) => match value {
            LoraValue::Point(p) => lora_store::bbox_contains(p, ll, ur) == Some(true),
            _ => false,
        },
    }
}

/// Candidate ids for one condition from an index on `(scope, property)`,
/// a superset of the matches; `None` when no index answers it.
fn index_candidates<S: GraphStorage>(
    storage: &S,
    entity: StoredIndexEntity,
    scope: &str,
    property: &str,
    condition: &Condition,
) -> Option<Vec<u64>> {
    let prop = |v: &LoraValue| lora_value_to_property(v.clone()).ok();
    let range = |lo: Option<&lora_store::PropertyValue>, hi: Option<&lora_store::PropertyValue>| {
        match entity {
            StoredIndexEntity::Node => storage.node_range_candidates(scope, property, lo, hi),
            StoredIndexEntity::Relationship => {
                storage.relationship_range_candidates(scope, property, lo, hi)
            }
        }
    };
    match condition {
        Condition::OneOf(values) => {
            let mut out = Vec::new();
            for v in values {
                if matches!(v, LoraValue::Null) {
                    continue;
                }
                let p = prop(v)?;
                out.extend(range(Some(&p), Some(&p))?);
            }
            out.sort_unstable();
            out.dedup();
            Some(out)
        }
        Condition::Range { lo, hi } => {
            let lo = match lo {
                Some((v, _)) => Some(prop(v)?),
                None => None,
            };
            let hi = match hi {
                Some((v, _)) => Some(prop(v)?),
                None => None,
            };
            range(lo.as_ref(), hi.as_ref())
        }
        Condition::BBox(ll, ur) => {
            let (ranges, n) = lora_store::bbox_x_ranges(ll, ur);
            let (lo_y, hi_y) = (ll.y.min(ur.y), ll.y.max(ur.y));
            let mut out = Vec::new();
            for (lo_x, hi_x) in &ranges[..n] {
                out.extend(match entity {
                    StoredIndexEntity::Node => storage.node_point_within_bbox(
                        scope,
                        property,
                        (*lo_x, lo_y),
                        (*hi_x, hi_y),
                    )?,
                    StoredIndexEntity::Relationship => storage.relationship_point_within_bbox(
                        scope,
                        property,
                        (*lo_x, lo_y),
                        (*hi_x, hi_y),
                    )?,
                });
            }
            out.sort_unstable();
            out.dedup();
            Some(out)
        }
    }
}

/// The options map of a vector query: `{restrictTo: [...], where: {...}}`.
#[derive(Debug, Clone, Default)]
pub struct VectorOptions {
    restrict_to: Option<BTreeSet<u64>>,
    filter: Option<VectorFilter>,
}

impl VectorOptions {
    /// Parses the fourth procedure argument; null means no options.
    pub fn parse(value: &LoraValue) -> Result<Self, String> {
        let options = match value {
            LoraValue::Null => return Ok(Self::default()),
            LoraValue::Map(options) => options,
            other => {
                return Err(format!(
                    "vector options must be a MAP like {{where: {{...}}}}, got {}",
                    kind(other)
                ))
            }
        };
        let mut out = Self::default();
        for (key, value) in options {
            match key.as_str() {
                "restrictTo" => out.restrict_to = Some(parse_ids(value)?),
                "where" => {
                    let filter = VectorFilter::parse(value)?;
                    out.filter = (!filter.is_empty()).then_some(filter);
                }
                other => {
                    return Err(format!(
                        "unknown option `{other}` for a vector query (known: `restrictTo`, `where`)"
                    ))
                }
            }
        }
        Ok(out)
    }

    /// The ids the search may return, `None` for every entity the index
    /// holds. `scope` is the index's label or relationship type.
    pub fn allowed_ids<S: GraphStorage>(
        &self,
        storage: &S,
        entity: StoredIndexEntity,
        scope: Option<&str>,
    ) -> Option<BTreeSet<u64>> {
        let matching = match (&self.filter, scope) {
            (Some(filter), Some(scope)) => Some(filter.matching_ids(storage, entity, scope)),
            _ => None,
        };
        match (&self.restrict_to, matching) {
            (Some(allowed), Some(matching)) => Some(
                matching
                    .into_iter()
                    .filter(|id| allowed.contains(id))
                    .collect(),
            ),
            (Some(allowed), None) => Some(allowed.clone()),
            (None, matching) => matching,
        }
    }
}

fn parse_ids(value: &LoraValue) -> Result<BTreeSet<u64>, String> {
    let LoraValue::List(items) = value else {
        return Err(format!(
            "`restrictTo` must be a LIST of ids, got {}",
            kind(value)
        ));
    };
    items
        .iter()
        .map(|item| match item {
            LoraValue::Int(n) if *n >= 0 => Ok(*n as u64),
            LoraValue::Node(id) | LoraValue::Relationship(id) => Ok(*id),
            other => Err(format!(
                "`restrictTo` entries must be ids, nodes or relationships, got {}",
                kind(other)
            )),
        })
        .collect()
}

/// The k best of `scored`, best first, ties by id: a partial selection,
/// not a sort of every scored entity.
pub fn top_k(mut scored: Vec<(u64, f64)>, k: usize) -> Vec<(u64, f64)> {
    let order = |a: &(u64, f64), b: &(u64, f64)| b.1.total_cmp(&a.1).then_with(|| a.0.cmp(&b.0));
    if k < scored.len() {
        if k == 0 {
            return Vec::new();
        }
        scored.select_nth_unstable_by(k - 1, order);
        scored.truncate(k);
    }
    scored.sort_by(order);
    scored
}
