//! Public-API-friendly mirror of a [`CompiledQuery`]'s operator tree.
//!
//! `PlanTree` walks the physical plan and produces a flat, serializable
//! description suitable for surfacing through `Database::explain` /
//! `Database::profile` and onwards through the language bindings.
//!
//! The internal `PhysicalOp` nodes reference analyzer-internal types
//! (`ResolvedExpr`, `VarId`); we deliberately do not expose those. Each
//! operator becomes a `PlanTreeNode` with an opaque, human-readable
//! `details` map keyed on stable strings. Future cost-modelling can fill
//! in `estimated_rows` without breaking the type.
use std::collections::BTreeMap;
use std::fmt::Write as _;

use lora_analyzer::{ResolvedExpr, ResolvedProjection};
use lora_ast::Direction;

use crate::physical::{PhysicalNodeId, PhysicalOp, PhysicalPlan};
use crate::{CompiledQuery, CompiledUnionBranch};

/// One node in the rendered plan tree.
#[derive(Debug, Clone)]
pub struct PlanTreeNode {
    /// Stable `PhysicalNodeId` within the owning plan. Synthetic
    /// nodes (e.g. the Union root, branch wrappers) reuse a sentinel
    /// id of `usize::MAX`.
    pub id: usize,
    /// Operator label, e.g. `NodeByLabelScan`, `Expand`, `Projection`.
    pub operator: String,
    /// Human-readable operator details. Values are stringified so the
    /// public API never leaks internal expression / `VarId` types.
    pub details: BTreeMap<String, String>,
    /// Reserved for a future cost model. Always `None` today.
    pub estimated_rows: Option<u64>,
    /// Children in physical execution order (leaf-most first).
    pub children: Vec<PlanTreeNode>,
}

/// Top-level plan tree.
#[derive(Debug, Clone)]
pub struct PlanTree {
    pub root: PlanTreeNode,
}

const SYNTHETIC_ID: usize = usize::MAX;

/// Build a `PlanTree` from a compiled query, including UNION branches.
pub fn plan_tree_from_compiled(compiled: &CompiledQuery) -> PlanTree {
    let head = build_node(&compiled.physical, compiled.physical.root);
    if compiled.unions.is_empty() {
        return PlanTree { root: head };
    }

    let mut children = Vec::with_capacity(compiled.unions.len() + 1);
    children.push(head);
    for branch in &compiled.unions {
        children.push(build_union_branch(branch));
    }
    let mut details = BTreeMap::new();
    details.insert("kind".to_string(), union_kind(&compiled.unions).to_string());
    PlanTree {
        root: PlanTreeNode {
            id: SYNTHETIC_ID,
            operator: "Union".to_string(),
            details,
            estimated_rows: None,
            children,
        },
    }
}

fn build_union_branch(branch: &CompiledUnionBranch) -> PlanTreeNode {
    let mut details = BTreeMap::new();
    details.insert(
        "kind".to_string(),
        if branch.all { "ALL" } else { "DISTINCT" }.to_string(),
    );
    PlanTreeNode {
        id: SYNTHETIC_ID,
        operator: "UnionBranch".to_string(),
        details,
        estimated_rows: None,
        children: vec![build_node(&branch.physical, branch.physical.root)],
    }
}

fn build_node(plan: &PhysicalPlan, id: PhysicalNodeId) -> PlanTreeNode {
    let op = &plan.nodes[id];
    let description = describe(op);
    let children = description
        .child_ids
        .into_iter()
        .map(|cid| build_node(plan, cid))
        .collect();
    PlanTreeNode {
        id,
        operator: description.operator,
        details: description.details,
        estimated_rows: None,
        children,
    }
}

struct PlanDescription {
    operator: String,
    details: BTreeMap<String, String>,
    child_ids: Vec<PhysicalNodeId>,
}

impl PlanDescription {
    fn leaf(operator: &str) -> Self {
        Self::new(operator, BTreeMap::new(), Vec::new())
    }

    fn with_children(
        operator: &str,
        details: BTreeMap<String, String>,
        child_ids: Vec<PhysicalNodeId>,
    ) -> Self {
        Self::new(operator, details, child_ids)
    }

    fn new(
        operator: &str,
        details: BTreeMap<String, String>,
        child_ids: Vec<PhysicalNodeId>,
    ) -> Self {
        Self {
            operator: operator.to_string(),
            details,
            child_ids,
        }
    }
}

fn describe(op: &PhysicalOp) -> PlanDescription {
    let mut d = BTreeMap::new();
    match op {
        PhysicalOp::Argument(_) => PlanDescription::leaf("Argument"),
        PhysicalOp::NodeScan(n) => {
            d.insert("var".to_string(), var_str(n.var));
            PlanDescription::with_children("NodeScan", d, opt_input(n.input))
        }
        PhysicalOp::NodeByLabelScan(n) => {
            d.insert("var".to_string(), var_str(n.var));
            d.insert("labels".to_string(), label_groups_str(&n.labels));
            PlanDescription::with_children("NodeByLabelScan", d, opt_input(n.input))
        }
        PhysicalOp::NodeByPropertyScan(n) => {
            d.insert("var".to_string(), var_str(n.var));
            if !n.labels.is_empty() {
                d.insert("labels".to_string(), label_groups_str(&n.labels));
            }
            d.insert("key".to_string(), n.key.clone());
            d.insert("value".to_string(), expr_str(&n.value));
            PlanDescription::with_children("NodeByPropertyScan", d, opt_input(n.input))
        }
        PhysicalOp::NodeByPropertyRangeScan(n) => {
            d.insert("var".to_string(), var_str(n.var));
            if !n.labels.is_empty() {
                d.insert("labels".to_string(), label_groups_str(&n.labels));
            }
            d.insert("key".to_string(), n.key.clone());
            if let Some(lo) = &n.lo {
                d.insert(
                    "lo".to_string(),
                    format!(
                        "{} {}",
                        if n.lo_inclusive { ">=" } else { ">" },
                        expr_str(lo)
                    ),
                );
            }
            if let Some(hi) = &n.hi {
                d.insert(
                    "hi".to_string(),
                    format!(
                        "{} {}",
                        if n.hi_inclusive { "<=" } else { "<" },
                        expr_str(hi)
                    ),
                );
            }
            PlanDescription::with_children("NodeByPropertyRangeScan", d, opt_input(n.input))
        }
        PhysicalOp::NodeByPointScan(n) => {
            d.insert("var".to_string(), var_str(n.var));
            if !n.labels.is_empty() {
                d.insert("labels".to_string(), label_groups_str(&n.labels));
            }
            d.insert("key".to_string(), n.key.clone());
            match &n.predicate {
                crate::PointPredicate::WithinBBox {
                    lower_left,
                    upper_right,
                } => {
                    d.insert("predicate".to_string(), "withinBBox".to_string());
                    d.insert("lowerLeft".to_string(), expr_str(lower_left));
                    d.insert("upperRight".to_string(), expr_str(upper_right));
                }
                crate::PointPredicate::WithinDistance {
                    center,
                    max_distance,
                    inclusive,
                } => {
                    d.insert(
                        "predicate".to_string(),
                        if *inclusive {
                            "distance<="
                        } else {
                            "distance<"
                        }
                        .to_string(),
                    );
                    d.insert("center".to_string(), expr_str(center));
                    d.insert("maxDistance".to_string(), expr_str(max_distance));
                }
            }
            PlanDescription::with_children("NodeByPointScan", d, opt_input(n.input))
        }
        PhysicalOp::NodeByTextScan(n) => {
            d.insert("var".to_string(), var_str(n.var));
            if !n.labels.is_empty() {
                d.insert("labels".to_string(), label_groups_str(&n.labels));
            }
            d.insert("key".to_string(), n.key.clone());
            d.insert(
                "predicate".to_string(),
                match n.predicate {
                    crate::TextPredicate::StartsWith => "STARTS WITH",
                    crate::TextPredicate::EndsWith => "ENDS WITH",
                    crate::TextPredicate::Contains => "CONTAINS",
                }
                .to_string(),
            );
            d.insert("query".to_string(), expr_str(&n.query));
            PlanDescription::with_children("NodeByTextScan", d, opt_input(n.input))
        }
        PhysicalOp::RelByPropertyRangeScan(n) => {
            d.insert("rel".to_string(), var_str(n.rel));
            d.insert("src".to_string(), var_str(n.src));
            d.insert("dst".to_string(), var_str(n.dst));
            if !n.types.is_empty() {
                d.insert("types".to_string(), n.types.join("|"));
            }
            d.insert(
                "direction".to_string(),
                direction_str(n.direction).to_string(),
            );
            d.insert("key".to_string(), n.key.clone());
            if let Some(lo) = &n.lo {
                d.insert(
                    "lo".to_string(),
                    format!(
                        "{} {}",
                        if n.lo_inclusive { ">=" } else { ">" },
                        expr_str(lo)
                    ),
                );
            }
            if let Some(hi) = &n.hi {
                d.insert(
                    "hi".to_string(),
                    format!(
                        "{} {}",
                        if n.hi_inclusive { "<=" } else { "<" },
                        expr_str(hi)
                    ),
                );
            }
            PlanDescription::with_children("RelByPropertyRangeScan", d, opt_input(n.input))
        }
        PhysicalOp::RelByTextScan(n) => {
            d.insert("rel".to_string(), var_str(n.rel));
            d.insert("src".to_string(), var_str(n.src));
            d.insert("dst".to_string(), var_str(n.dst));
            if !n.types.is_empty() {
                d.insert("types".to_string(), n.types.join("|"));
            }
            d.insert(
                "direction".to_string(),
                direction_str(n.direction).to_string(),
            );
            d.insert("key".to_string(), n.key.clone());
            d.insert(
                "predicate".to_string(),
                match n.predicate {
                    crate::TextPredicate::StartsWith => "STARTS WITH",
                    crate::TextPredicate::EndsWith => "ENDS WITH",
                    crate::TextPredicate::Contains => "CONTAINS",
                }
                .to_string(),
            );
            d.insert("query".to_string(), expr_str(&n.query));
            PlanDescription::with_children("RelByTextScan", d, opt_input(n.input))
        }
        PhysicalOp::RelByPointScan(n) => {
            d.insert("rel".to_string(), var_str(n.rel));
            d.insert("src".to_string(), var_str(n.src));
            d.insert("dst".to_string(), var_str(n.dst));
            if !n.types.is_empty() {
                d.insert("types".to_string(), n.types.join("|"));
            }
            d.insert(
                "direction".to_string(),
                direction_str(n.direction).to_string(),
            );
            d.insert("key".to_string(), n.key.clone());
            match &n.predicate {
                crate::PointPredicate::WithinBBox {
                    lower_left,
                    upper_right,
                } => {
                    d.insert("predicate".to_string(), "withinBBox".to_string());
                    d.insert("lowerLeft".to_string(), expr_str(lower_left));
                    d.insert("upperRight".to_string(), expr_str(upper_right));
                }
                crate::PointPredicate::WithinDistance {
                    center,
                    max_distance,
                    inclusive,
                } => {
                    d.insert(
                        "predicate".to_string(),
                        if *inclusive {
                            "distance<="
                        } else {
                            "distance<"
                        }
                        .to_string(),
                    );
                    d.insert("center".to_string(), expr_str(center));
                    d.insert("maxDistance".to_string(), expr_str(max_distance));
                }
            }
            PlanDescription::with_children("RelByPointScan", d, opt_input(n.input))
        }
        PhysicalOp::Expand(n) => describe_expand(n),
        PhysicalOp::Filter(n) => {
            d.insert("predicate".to_string(), expr_str(&n.predicate));
            PlanDescription::with_children("Filter", d, vec![n.input])
        }
        PhysicalOp::Projection(n) => describe_projection(n),
        PhysicalOp::Unwind(n) => {
            d.insert("alias".to_string(), var_str(n.alias));
            d.insert("expr".to_string(), expr_str(&n.expr));
            PlanDescription::with_children("Unwind", d, vec![n.input])
        }
        PhysicalOp::HashAggregation(n) => describe_hash_aggregation(n),
        PhysicalOp::Sort(n) => {
            d.insert(
                "items".to_string(),
                format!("{} sort key(s)", n.items.len()),
            );
            if let Some(top_k) = n.top_k {
                d.insert("top_k".to_string(), top_k.to_string());
            }
            PlanDescription::with_children("Sort", d, vec![n.input])
        }
        PhysicalOp::Limit(n) => {
            if let Some(skip) = &n.skip {
                d.insert("skip".to_string(), expr_str(skip));
            }
            if let Some(limit) = &n.limit {
                d.insert("limit".to_string(), expr_str(limit));
            }
            PlanDescription::with_children("Limit", d, vec![n.input])
        }
        PhysicalOp::Create(n) => {
            d.insert(
                "elements".to_string(),
                pattern_summary(n.pattern.parts.len()),
            );
            PlanDescription::with_children("Create", d, vec![n.input])
        }
        PhysicalOp::Merge(n) => describe_merge(n),
        PhysicalOp::Delete(n) => {
            d.insert("detach".to_string(), n.detach.to_string());
            d.insert("targets".to_string(), n.expressions.len().to_string());
            PlanDescription::with_children("Delete", d, vec![n.input])
        }
        PhysicalOp::Set(n) => {
            d.insert("items".to_string(), n.items.len().to_string());
            PlanDescription::with_children("Set", d, vec![n.input])
        }
        PhysicalOp::Remove(n) => {
            d.insert("items".to_string(), n.items.len().to_string());
            PlanDescription::with_children("Remove", d, vec![n.input])
        }
        PhysicalOp::Foreach(n) => {
            d.insert("variable".to_string(), var_str(n.variable));
            d.insert("list".to_string(), expr_str(&n.list));
            d.insert("body".to_string(), n.body.len().to_string());
            PlanDescription::with_children("Foreach", d, vec![n.input])
        }
        PhysicalOp::OptionalMatch(n) => describe_optional_match(n),
        PhysicalOp::CallSubquery(n) => {
            d.insert(
                "new_vars".to_string(),
                n.new_vars
                    .iter()
                    .copied()
                    .map(var_str)
                    .collect::<Vec<_>>()
                    .join(", "),
            );
            PlanDescription::with_children("CallSubquery", d, vec![n.input, n.inner])
        }
        PhysicalOp::PathBuild(n) => {
            d.insert("output".to_string(), var_str(n.output));
            d.insert("nodes".to_string(), n.node_vars.len().to_string());
            d.insert("rels".to_string(), n.rel_vars.len().to_string());
            if let Some(all) = n.shortest_path_all {
                d.insert("shortest_path_all".to_string(), all.to_string());
            }
            PlanDescription::with_children("PathBuild", d, vec![n.input])
        }
    }
}

fn union_kind(branches: &[CompiledUnionBranch]) -> &'static str {
    let all = branches.iter().all(|b| b.all);
    let all_distinct = branches.iter().all(|b| !b.all);
    if all {
        "ALL"
    } else if all_distinct {
        "DISTINCT"
    } else {
        "MIXED"
    }
}

fn describe_expand(n: &crate::physical::ExpandExec) -> PlanDescription {
    let mut d = BTreeMap::new();
    d.insert("src".to_string(), var_str(n.src));
    d.insert("dst".to_string(), var_str(n.dst));
    if let Some(rel) = n.rel {
        d.insert("rel".to_string(), var_str(rel));
    }
    if !n.types.is_empty() {
        d.insert("types".to_string(), n.types.join("|"));
    }
    d.insert(
        "direction".to_string(),
        direction_str(n.direction).to_string(),
    );
    if let Some(props) = &n.rel_properties {
        d.insert("rel_properties".to_string(), expr_str(props));
    }
    if let Some(range) = &n.range {
        d.insert("range".to_string(), format!("{:?}", range));
    }
    PlanDescription::with_children("Expand", d, vec![n.input])
}

fn describe_projection(n: &crate::physical::ProjectionExec) -> PlanDescription {
    let mut d = BTreeMap::new();
    d.insert("distinct".to_string(), n.distinct.to_string());
    d.insert(
        "include_existing".to_string(),
        n.include_existing.to_string(),
    );
    d.insert("items".to_string(), projection_names(&n.items));
    PlanDescription::with_children("Projection", d, vec![n.input])
}

fn describe_hash_aggregation(n: &crate::physical::HashAggregationExec) -> PlanDescription {
    let mut d = BTreeMap::new();
    d.insert("group_by".to_string(), projection_names(&n.group_by));
    d.insert("aggregates".to_string(), projection_names(&n.aggregates));
    PlanDescription::with_children("HashAggregation", d, vec![n.input])
}

fn describe_merge(n: &crate::physical::MergeExec) -> PlanDescription {
    let mut d = BTreeMap::new();
    d.insert("actions".to_string(), n.actions.len().to_string());
    PlanDescription::with_children("Merge", d, vec![n.input])
}

fn describe_optional_match(n: &crate::physical::OptionalMatchExec) -> PlanDescription {
    let mut d = BTreeMap::new();
    d.insert(
        "new_vars".to_string(),
        n.new_vars
            .iter()
            .copied()
            .map(var_str)
            .collect::<Vec<_>>()
            .join(", "),
    );
    PlanDescription::with_children("OptionalMatch", d, vec![n.input, n.inner])
}

fn opt_input(input: Option<PhysicalNodeId>) -> Vec<PhysicalNodeId> {
    input.map(|i| vec![i]).unwrap_or_default()
}

fn var_str(v: lora_analyzer::symbols::VarId) -> String {
    format!("v{}", v.0)
}

fn label_groups_str(groups: &[Vec<String>]) -> String {
    groups
        .iter()
        .map(|or_group| or_group.join("|"))
        .collect::<Vec<_>>()
        .join("&")
}

fn projection_names(items: &[ResolvedProjection]) -> String {
    items
        .iter()
        .map(|p| p.name.clone())
        .collect::<Vec<_>>()
        .join(", ")
}

fn direction_str(d: Direction) -> &'static str {
    match d {
        Direction::Right => "->",
        Direction::Left => "<-",
        Direction::Undirected => "-",
    }
}

fn expr_str(e: &ResolvedExpr) -> String {
    let mut out = String::new();
    let _ = write!(&mut out, "{:?}", e);
    out
}

fn pattern_summary(part_count: usize) -> String {
    format!("{} pattern part(s)", part_count)
}
