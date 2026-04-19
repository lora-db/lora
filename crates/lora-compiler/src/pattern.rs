use crate::logical::*;
use crate::planner::Planner;
use lora_analyzer::{
    ResolvedChain, ResolvedExpr, ResolvedPattern, ResolvedPatternElement, ResolvedPatternPart,
};
use lora_ast::BinaryOp;

pub struct PatternPlanner<'a> {
    planner: &'a mut Planner,
}

impl<'a> PatternPlanner<'a> {
    pub fn new(planner: &'a mut Planner) -> Self {
        Self { planner }
    }

    pub fn plan_pattern(
        &mut self,
        input: Option<PlanNodeId>,
        pattern: &ResolvedPattern,
    ) -> PlanNodeId {
        let mut last = input;

        for part in &pattern.parts {
            last = Some(self.plan_part(last, part));
        }

        last.expect("pattern produced no plan")
    }

    fn plan_part(
        &mut self,
        input: Option<PlanNodeId>,
        part: &ResolvedPatternPart,
    ) -> PlanNodeId {
        let shortest_path_all = match &part.element {
            ResolvedPatternElement::ShortestPath { all, .. } => Some(*all),
            _ => None,
        };

        let node = self.plan_element(input, &part.element);

        // If the pattern part has a path binding, add a PathBuild operator.
        if let Some(path_var) = part.binding {
            let (node_vars, rel_vars) = collect_chain_vars(&part.element);
            if !node_vars.is_empty() {
                return self.planner.push(LogicalOp::PathBuild(PathBuild {
                    input: node,
                    output: path_var,
                    node_vars,
                    rel_vars,
                    shortest_path_all,
                }));
            }
        }

        node
    }

    fn plan_element(
        &mut self,
        input: Option<PlanNodeId>,
        el: &ResolvedPatternElement,
    ) -> PlanNodeId {
        match el {
            ResolvedPatternElement::Node {
                var,
                labels,
                properties, // ← was `properties: _`, now used
            } => self.plan_node(input, *var, labels, properties.as_ref()),

            ResolvedPatternElement::ShortestPath { head, chain, .. } |
            ResolvedPatternElement::NodeChain { head, chain } => {
                let mut node = self.plan_node(input, head.var, &head.labels, head.properties.as_ref());
                // The analyzer always assigns a VarId (even for anonymous nodes),
                // so .unwrap() is safe here.
                let mut current_src = head.var.unwrap();

                for step in chain {
                    let dst = step.node.var.unwrap();

                    node = self.plan_expand(node, current_src, dst, step);

                    // Filter inline properties on chain step nodes too
                    if let Some(props) = step.node.properties.as_ref() {
                        if let Some(predicate) = build_property_predicate(dst, props) {
                            node = self.planner.push(LogicalOp::Filter(Filter {
                                input: node,
                                predicate,
                            }));
                        }
                    }

                    current_src = dst;
                }

                node
            }
        }
    }

    fn plan_node(
        &mut self,
        input: Option<PlanNodeId>,
        var: Option<lora_analyzer::symbols::VarId>,
        labels: &[Vec<String>],
        properties: Option<&ResolvedExpr>,
    ) -> PlanNodeId {
        // The analyzer always assigns a VarId (even for anonymous nodes).
        let var = var.unwrap();

        let mut node = self.planner.push(LogicalOp::NodeScan(NodeScan {
            input,
            var,
            labels: labels.to_vec(),
        }));

        // Emit a Filter for any inline property predicates e.g. (a:User {id: 5})
        if let Some(props) = properties {
            if let Some(predicate) = build_property_predicate(var, props) {
                node = self.planner.push(LogicalOp::Filter(Filter {
                    input: node,
                    predicate,
                }));
            }
        }

        node
    }

    fn plan_expand(
        &mut self,
        input: PlanNodeId,
        src: lora_analyzer::symbols::VarId,
        dst: lora_analyzer::symbols::VarId,
        step: &ResolvedChain,
    ) -> PlanNodeId {
        self.planner.push(LogicalOp::Expand(Expand {
            input,
            src,
            rel: step.rel.var,
            dst,
            types: step.rel.types.clone(),
            direction: step.rel.direction,
            rel_properties: step.rel.properties.clone(),
            range: step.rel.range.clone(),
        }))
    }
}

/// Extract node and relationship VarIds from a pattern element for path construction.
fn collect_chain_vars(
    el: &ResolvedPatternElement,
) -> (Vec<lora_analyzer::symbols::VarId>, Vec<lora_analyzer::symbols::VarId>) {
    match el {
        ResolvedPatternElement::Node { var, .. } => {
            let node_vars = var.iter().copied().collect();
            (node_vars, Vec::new())
        }
        ResolvedPatternElement::ShortestPath { head, chain, .. } |
        ResolvedPatternElement::NodeChain { head, chain } => {
            let mut node_vars = Vec::new();
            let mut rel_vars = Vec::new();

            if let Some(v) = head.var {
                node_vars.push(v);
            }

            for step in chain {
                if let Some(v) = step.rel.var {
                    rel_vars.push(v);
                }
                if let Some(v) = step.node.var {
                    node_vars.push(v);
                }
            }

            (node_vars, rel_vars)
        }
    }
}

fn build_property_predicate(
    var_id: lora_analyzer::symbols::VarId,
    props_expr: &ResolvedExpr,
) -> Option<ResolvedExpr> {
    let ResolvedExpr::Map(pairs) = props_expr else {
        return None;
    };

    let mut predicate: Option<ResolvedExpr> = None;

    for (key, value_expr) in pairs {
        let prop_access = ResolvedExpr::Property {
            expr: Box::new(ResolvedExpr::Variable(var_id)),
            property: key.clone(),
        };

        let eq = ResolvedExpr::Binary {
            lhs: Box::new(prop_access),
            op: BinaryOp::Eq,
            rhs: Box::new(value_expr.clone()),
        };

        predicate = Some(match predicate {
            None => eq,
            Some(existing) => ResolvedExpr::Binary {
                lhs: Box::new(existing),
                op: BinaryOp::And,
                rhs: Box::new(eq),
            },
        });
    }

    predicate
}