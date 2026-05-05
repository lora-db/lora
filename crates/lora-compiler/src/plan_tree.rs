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

use lora_analyzer::ResolvedExpr;
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
    let all = compiled.unions.iter().all(|b| b.all);
    let any_distinct = compiled.unions.iter().any(|b| !b.all);
    let kind = if all && !any_distinct {
        "ALL"
    } else if any_distinct && compiled.unions.iter().all(|b| !b.all) {
        "DISTINCT"
    } else {
        "MIXED"
    };
    details.insert("kind".to_string(), kind.to_string());
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
    let (operator, details, child_ids) = describe(op);
    let children = child_ids
        .into_iter()
        .map(|cid| build_node(plan, cid))
        .collect();
    PlanTreeNode {
        id,
        operator,
        details,
        estimated_rows: None,
        children,
    }
}

fn describe(op: &PhysicalOp) -> (String, BTreeMap<String, String>, Vec<PhysicalNodeId>) {
    let mut d = BTreeMap::new();
    match op {
        PhysicalOp::Argument(_) => ("Argument".to_string(), d, Vec::new()),
        PhysicalOp::NodeScan(n) => {
            d.insert("var".to_string(), var_str(n.var));
            ("NodeScan".to_string(), d, opt_input(n.input))
        }
        PhysicalOp::NodeByLabelScan(n) => {
            d.insert("var".to_string(), var_str(n.var));
            d.insert("labels".to_string(), label_groups_str(&n.labels));
            ("NodeByLabelScan".to_string(), d, opt_input(n.input))
        }
        PhysicalOp::NodeByPropertyScan(n) => {
            d.insert("var".to_string(), var_str(n.var));
            if !n.labels.is_empty() {
                d.insert("labels".to_string(), label_groups_str(&n.labels));
            }
            d.insert("key".to_string(), n.key.clone());
            d.insert("value".to_string(), expr_str(&n.value));
            ("NodeByPropertyScan".to_string(), d, opt_input(n.input))
        }
        PhysicalOp::Expand(n) => {
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
            ("Expand".to_string(), d, vec![n.input])
        }
        PhysicalOp::Filter(n) => {
            d.insert("predicate".to_string(), expr_str(&n.predicate));
            ("Filter".to_string(), d, vec![n.input])
        }
        PhysicalOp::Projection(n) => {
            d.insert("distinct".to_string(), n.distinct.to_string());
            d.insert(
                "include_existing".to_string(),
                n.include_existing.to_string(),
            );
            d.insert(
                "items".to_string(),
                n.items
                    .iter()
                    .map(|p| p.name.clone())
                    .collect::<Vec<_>>()
                    .join(", "),
            );
            ("Projection".to_string(), d, vec![n.input])
        }
        PhysicalOp::Unwind(n) => {
            d.insert("alias".to_string(), var_str(n.alias));
            d.insert("expr".to_string(), expr_str(&n.expr));
            ("Unwind".to_string(), d, vec![n.input])
        }
        PhysicalOp::HashAggregation(n) => {
            d.insert(
                "group_by".to_string(),
                n.group_by
                    .iter()
                    .map(|p| p.name.clone())
                    .collect::<Vec<_>>()
                    .join(", "),
            );
            d.insert(
                "aggregates".to_string(),
                n.aggregates
                    .iter()
                    .map(|p| p.name.clone())
                    .collect::<Vec<_>>()
                    .join(", "),
            );
            ("HashAggregation".to_string(), d, vec![n.input])
        }
        PhysicalOp::Sort(n) => {
            d.insert(
                "items".to_string(),
                format!("{} sort key(s)", n.items.len()),
            );
            ("Sort".to_string(), d, vec![n.input])
        }
        PhysicalOp::Limit(n) => {
            if let Some(skip) = &n.skip {
                d.insert("skip".to_string(), expr_str(skip));
            }
            if let Some(limit) = &n.limit {
                d.insert("limit".to_string(), expr_str(limit));
            }
            ("Limit".to_string(), d, vec![n.input])
        }
        PhysicalOp::Create(n) => {
            d.insert(
                "elements".to_string(),
                pattern_summary(n.pattern.parts.len()),
            );
            ("Create".to_string(), d, vec![n.input])
        }
        PhysicalOp::Merge(n) => {
            d.insert(
                "actions".to_string(),
                if n.actions.is_empty() {
                    "0".to_string()
                } else {
                    n.actions.len().to_string()
                },
            );
            let _ = &n.pattern_part;
            ("Merge".to_string(), d, vec![n.input])
        }
        PhysicalOp::Delete(n) => {
            d.insert("detach".to_string(), n.detach.to_string());
            d.insert("targets".to_string(), n.expressions.len().to_string());
            ("Delete".to_string(), d, vec![n.input])
        }
        PhysicalOp::Set(n) => {
            d.insert("items".to_string(), n.items.len().to_string());
            ("Set".to_string(), d, vec![n.input])
        }
        PhysicalOp::Remove(n) => {
            d.insert("items".to_string(), n.items.len().to_string());
            ("Remove".to_string(), d, vec![n.input])
        }
        PhysicalOp::OptionalMatch(n) => {
            d.insert(
                "new_vars".to_string(),
                n.new_vars
                    .iter()
                    .copied()
                    .map(var_str)
                    .collect::<Vec<_>>()
                    .join(", "),
            );
            ("OptionalMatch".to_string(), d, vec![n.input, n.inner])
        }
        PhysicalOp::PathBuild(n) => {
            d.insert("output".to_string(), var_str(n.output));
            d.insert("nodes".to_string(), n.node_vars.len().to_string());
            d.insert("rels".to_string(), n.rel_vars.len().to_string());
            if let Some(all) = n.shortest_path_all {
                d.insert("shortest_path_all".to_string(), all.to_string());
            }
            ("PathBuild".to_string(), d, vec![n.input])
        }
    }
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
