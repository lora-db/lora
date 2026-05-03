use super::state::{Analyzer, PatternContext};
use crate::{errors::*, resolved::*};
use lora_ast::{NodePattern, Pattern, PatternElement, PatternPart, RelationshipPattern};
use lora_store::GraphCatalog;
use std::collections::{BTreeMap, BTreeSet};

impl<'a, S: GraphCatalog + ?Sized> Analyzer<'a, S> {
    pub(super) fn analyze_pattern(
        &mut self,
        p: &Pattern,
        context: PatternContext,
    ) -> Result<ResolvedPattern, SemanticError> {
        let mut parts = Vec::with_capacity(p.parts.len());

        // In read patterns, detect when the same node variable is used at
        // multiple positions with conflicting labels (e.g. (n:X)-[r]->(n:Y)).
        if matches!(context, PatternContext::Read | PatternContext::OptionalRead) {
            let mut node_labels: BTreeMap<String, Vec<String>> = BTreeMap::new();
            for part in &p.parts {
                self.collect_node_var_labels(&part.element, &mut node_labels);
            }
            for (name, labels_list) in &node_labels {
                // Only reject if the variable appears with distinct non-empty label sets
                if labels_list.len() > 1 {
                    let non_empty: Vec<&String> =
                        labels_list.iter().filter(|l| !l.is_empty()).collect();
                    let unique_labels: BTreeSet<&String> = non_empty.iter().copied().collect();
                    if unique_labels.len() > 1 {
                        return Err(SemanticError::DuplicateVariable(name.clone()));
                    }
                }
            }
        }

        for part in &p.parts {
            parts.push(self.analyze_pattern_part(part, context)?);
        }

        Ok(ResolvedPattern { parts })
    }

    /// Collect (variable_name, labels_string) for each node position in a pattern element.
    fn collect_node_var_labels(
        &self,
        el: &PatternElement,
        map: &mut BTreeMap<String, Vec<String>>,
    ) {
        match el {
            PatternElement::NodeChain { head, chain, .. } => {
                if let Some(ref v) = head.variable {
                    let label_str = format_label_groups(&head.labels);
                    map.entry(v.name.clone()).or_default().push(label_str);
                }
                for step in chain {
                    if let Some(ref v) = step.node.variable {
                        let label_str = format_label_groups(&step.node.labels);
                        map.entry(v.name.clone()).or_default().push(label_str);
                    }
                }
            }
            PatternElement::Parenthesized(inner, _) => {
                self.collect_node_var_labels(inner, map);
            }
            PatternElement::ShortestPath { element, .. } => {
                self.collect_node_var_labels(element, map);
            }
        }
    }

    pub(super) fn analyze_pattern_part(
        &mut self,
        part: &PatternPart,
        context: PatternContext,
    ) -> Result<ResolvedPatternPart, SemanticError> {
        let binding = part
            .binding
            .as_ref()
            .map(|v| self.declare_or_reuse_variable(&v.name))
            .transpose()?;

        let element = self.analyze_pattern_element(&part.element, context)?;

        Ok(ResolvedPatternPart { binding, element })
    }

    fn analyze_pattern_element(
        &mut self,
        el: &PatternElement,
        context: PatternContext,
    ) -> Result<ResolvedPatternElement, SemanticError> {
        match el {
            PatternElement::NodeChain { head, chain, .. } => {
                if chain.is_empty() {
                    let node = self.analyze_node(head, context)?;
                    return Ok(ResolvedPatternElement::Node {
                        var: node.var,
                        labels: node.labels,
                        properties: node.properties,
                    });
                }

                let head = self.analyze_node(head, context)?;
                let mut resolved_chain = Vec::with_capacity(chain.len());

                for step in chain {
                    let rel = self.analyze_relationship(&step.relationship, context)?;
                    let node = self.analyze_node(&step.node, context)?;
                    resolved_chain.push(ResolvedChain { rel, node });
                }

                Ok(ResolvedPatternElement::NodeChain {
                    head,
                    chain: resolved_chain,
                })
            }

            PatternElement::Parenthesized(inner, _) => self.analyze_pattern_element(inner, context),

            PatternElement::ShortestPath { all, element, .. } => {
                let resolved = self.analyze_pattern_element(element, context)?;
                match resolved {
                    ResolvedPatternElement::NodeChain { head, chain } => {
                        Ok(ResolvedPatternElement::ShortestPath {
                            all: *all,
                            head,
                            chain,
                        })
                    }
                    other => Ok(other),
                }
            }
        }
    }

    fn analyze_node(
        &mut self,
        node: &NodePattern,
        context: PatternContext,
    ) -> Result<ResolvedNode, SemanticError> {
        let var = Some(match &node.variable {
            // Named node — declare in scope so user code can reference it.
            Some(v) => self.declare_or_reuse_variable(&v.name)?,
            // Anonymous node (e.g. `(:Person)`) — allocate an internal VarId
            // but do NOT declare it in the scope, so it cannot be referenced
            // by user expressions and will not appear in projections.
            None => self.symbols.new_var(),
        });

        let labels: Vec<Vec<String>> = node
            .labels
            .iter()
            .map(|group| {
                group
                    .iter()
                    .map(|l| {
                        self.validate_label_name(l, context)?;
                        Ok(l.clone())
                    })
                    .collect::<Result<Vec<_>, SemanticError>>()
            })
            .collect::<Result<Vec<_>, SemanticError>>()?;

        let properties = node
            .properties
            .as_ref()
            .map(|e| self.analyze_property_map_expr(e))
            .transpose()?;

        Ok(ResolvedNode {
            var,
            labels,
            properties,
        })
    }

    fn analyze_relationship(
        &mut self,
        rel: &RelationshipPattern,
        context: PatternContext,
    ) -> Result<ResolvedRel, SemanticError> {
        if let Some(detail) = &rel.detail {
            let var = Some(match &detail.variable {
                Some(v) => self.declare_or_reuse_variable(&v.name)?,
                // Anonymous relationship — allocate an internal VarId so the
                // relationship value is stored in the row (needed for path
                // materialization).
                None => self.symbols.new_var(),
            });

            let types = detail
                .types
                .iter()
                .map(|t| {
                    self.validate_relationship_type_name(t, context)?;
                    Ok(t.clone())
                })
                .collect::<Result<Vec<_>, SemanticError>>()?;

            if let Some(range) = &detail.range {
                if let (Some(start), Some(end)) = (range.start, range.end) {
                    if start > end {
                        return Err(SemanticError::InvalidRange(
                            start,
                            end,
                            range.span.start,
                            range.span.end,
                        ));
                    }
                }
            }

            let properties = detail
                .properties
                .as_ref()
                .map(|e| self.analyze_property_map_expr(e))
                .transpose()?;

            Ok(ResolvedRel {
                var,
                types,
                direction: rel.direction,
                range: detail.range.clone(),
                properties,
            })
        } else {
            Ok(ResolvedRel {
                var: None,
                types: Vec::new(),
                direction: rel.direction,
                range: None,
                properties: None,
            })
        }
    }
}

/// Format label groups as a string for duplicate-variable detection.
fn format_label_groups(groups: &[impl AsRef<[String]>]) -> String {
    groups
        .iter()
        .map(|g| g.as_ref().join("|"))
        .collect::<Vec<_>>()
        .join(":")
}
