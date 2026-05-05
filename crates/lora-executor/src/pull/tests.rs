use std::collections::BTreeMap;
use std::sync::Arc;

use lora_analyzer::symbols::VarId;
use lora_ast::{Direction, RangeLiteral, Span};
use lora_compiler::physical::{
    ArgumentExec, ExpandExec, NodeByLabelScanExec, PhysicalOp, PhysicalPlan,
};
use lora_store::{GraphStorageMut, InMemoryGraph};

use crate::value::LoraValue;

use super::{build_streaming, drain, subtree_is_fully_streaming};

#[test]
fn variable_length_expand_has_streaming_source() {
    let mut graph = InMemoryGraph::new();
    let a = graph.create_node(vec!["N".into()], BTreeMap::new());
    let b = graph.create_node(vec!["N".into()], BTreeMap::new());
    let c = graph.create_node(vec!["N".into()], BTreeMap::new());
    graph
        .create_relationship(a.id, b.id, "R", BTreeMap::new())
        .unwrap();
    graph
        .create_relationship(b.id, c.id, "R", BTreeMap::new())
        .unwrap();

    let src = VarId(0);
    let rel = VarId(1);
    let dst = VarId(2);
    let plan = PhysicalPlan {
        root: 2,
        nodes: vec![
            PhysicalOp::Argument(ArgumentExec),
            PhysicalOp::NodeByLabelScan(NodeByLabelScanExec {
                input: Some(0),
                var: src,
                labels: vec![vec!["N".into()]],
            }),
            PhysicalOp::Expand(ExpandExec {
                input: 1,
                src,
                rel: Some(rel),
                dst,
                types: vec!["R".into()],
                direction: Direction::Right,
                rel_properties: None,
                range: Some(RangeLiteral {
                    start: Some(1),
                    end: Some(2),
                    span: Span::default(),
                }),
            }),
        ],
    };

    assert!(subtree_is_fully_streaming(&plan, plan.root));

    let mut source = build_streaming(&plan, plan.root, &graph, Arc::new(BTreeMap::new())).unwrap();
    let rows = drain(source.as_mut()).unwrap();
    let mut rel_lengths = rows
        .iter()
        .map(|row| match row.get(rel).unwrap() {
            LoraValue::List(rels) => rels.len(),
            other => panic!("expected relationship list, got {other:?}"),
        })
        .collect::<Vec<_>>();
    rel_lengths.sort_unstable();

    assert_eq!(rows.len(), 3);
    assert_eq!(rel_lengths, vec![1, 1, 2]);
}
