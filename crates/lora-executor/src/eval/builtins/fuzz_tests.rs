//! Calls every builtin with extreme arguments and asserts none panics or
//! runs away.
//!
//! In release builds the engine aborts on panic, so a builtin that panics on
//! some input (`f64::clamp` with reversed bounds, a formatting precision
//! over 65535, a capacity overflow) takes the whole process down. Each
//! builtin may return null or fail with an error for these inputs; it must
//! not panic, and must answer quickly instead of allocating without bound.

use std::collections::BTreeMap;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::time::{Duration, Instant};

use lora_analyzer::BUILTIN_SPECS;
use lora_store::InMemoryGraph;

use super::super::errors::clear_eval_error;
use super::super::expr::EvalContext;
use super::dispatch;
use crate::value::LoraValue;

fn s(text: &str) -> LoraValue {
    LoraValue::String(text.to_string())
}

fn list(items: Vec<LoraValue>) -> LoraValue {
    LoraValue::List(items)
}

fn map(entries: &[(&str, LoraValue)]) -> LoraValue {
    LoraValue::Map(
        entries
            .iter()
            .map(|(k, v)| (k.to_string(), v.clone()))
            .collect(),
    )
}

fn nested(depth: usize) -> LoraValue {
    let mut v = LoraValue::Int(1);
    for _ in 0..depth {
        v = list(vec![v]);
    }
    v
}

/// Values every argument slot is tried with.
fn extremes() -> Vec<LoraValue> {
    vec![
        LoraValue::Null,
        LoraValue::Bool(true),
        LoraValue::Int(0),
        LoraValue::Int(-1),
        LoraValue::Int(3),
        LoraValue::Int(1000),
        LoraValue::Int(i64::MIN),
        LoraValue::Int(i64::MAX),
        LoraValue::Float(f64::NAN),
        LoraValue::Float(f64::INFINITY),
        LoraValue::Float(-1e300),
        LoraValue::Float(9e18),
        LoraValue::Float(1.5),
        s(""),
        s("abc"),
        s("x".repeat(100_000).as_str()),
        s("(a+)+$"),
        s("/\\w{1000}{1000}/"),
        s("2024-02-30T25:61:61"),
        s("héllo wörld ✓"),
        list(vec![]),
        list(vec![
            LoraValue::Int(1),
            s("a"),
            LoraValue::Null,
            LoraValue::Float(f64::NAN),
        ]),
        list((0..64).map(LoraValue::Int).collect()),
        nested(crate::limits::MAX_VALUE_NESTING),
        map(&[]),
        map(&[
            ("years", LoraValue::Float(1e300)),
            ("months", LoraValue::Int(i64::MAX)),
            ("days", LoraValue::Int(i64::MIN)),
        ]),
        map(&[
            ("latitude", LoraValue::Int(1000)),
            ("longitude", LoraValue::Float(1e300)),
        ]),
        map(&[
            ("year", LoraValue::Int(i64::MAX)),
            ("month", LoraValue::Int(13)),
            ("epochSeconds", LoraValue::Int(i64::MAX)),
        ]),
        LoraValue::Node(u64::MAX),
        LoraValue::Duration(Box::new(lora_store::LoraDuration {
            months: i64::MAX,
            days: i64::MIN,
            seconds: i64::MAX,
            nanoseconds: 999_999_999,
        })),
        LoraValue::Binary(Box::new(lora_store::LoraBinary::from_bytes(vec![0xff; 64]))),
    ]
}

/// The few values pairs of slots are tried with together.
fn pair_values() -> Vec<LoraValue> {
    vec![
        LoraValue::Null,
        LoraValue::Int(-1),
        LoraValue::Int(i64::MAX),
        LoraValue::Float(f64::NAN),
        s(""),
        s("x".repeat(100_000).as_str()),
        list((0..64).map(LoraValue::Int).collect()),
        nested(crate::limits::MAX_VALUE_NESTING),
    ]
}

/// Values every slot starts from before one or two are varied.
fn bases() -> Vec<LoraValue> {
    vec![
        LoraValue::Null,
        LoraValue::Int(2),
        LoraValue::Float(1.5),
        s("ab"),
        list(vec![
            LoraValue::Int(1),
            LoraValue::Int(2),
            LoraValue::Int(3),
        ]),
        map(&[("a", LoraValue::Int(1))]),
    ]
}

#[test]
fn no_builtin_panics_or_runs_away_on_extreme_arguments() {
    // Unoptimised builds need room for the deepest values.
    std::thread::Builder::new()
        .stack_size(64 << 20)
        .spawn(run)
        .unwrap()
        .join()
        .unwrap();
}

fn run() {
    let graph = InMemoryGraph::new();
    let params = BTreeMap::new();
    let ctx = EvalContext {
        storage: &graph,
        params: &params,
    };
    let extremes = extremes();
    let pairs = pair_values();
    let bases = bases();
    let slow = Duration::from_secs(5);

    // Silence the default hook: the failures are reported below.
    let hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(|_| {}));

    let mut failures: Vec<String> = Vec::new();
    let mut calls = 0usize;
    for spec in BUILTIN_SPECS {
        let max = spec.arity.max.unwrap_or(spec.arity.min + 2).min(4);
        for arity in spec.arity.min..=max.max(spec.arity.min) {
            let mut argument_lists: Vec<Vec<LoraValue>> = Vec::new();
            for base in &bases {
                let args = vec![base.clone(); arity];
                argument_lists.push(args.clone());
                for slot in 0..arity {
                    for value in &extremes {
                        let mut varied = args.clone();
                        varied[slot] = value.clone();
                        argument_lists.push(varied);
                    }
                    for other in slot + 1..arity {
                        for a in &pairs {
                            for b in &pairs {
                                let mut varied = args.clone();
                                varied[slot] = a.clone();
                                varied[other] = b.clone();
                                argument_lists.push(varied);
                            }
                        }
                    }
                }
            }
            for args in argument_lists {
                calls += 1;
                clear_eval_error();
                let start = Instant::now();
                let outcome = catch_unwind(AssertUnwindSafe(|| dispatch(spec.name, &args, &ctx)));
                let took = start.elapsed();
                let shown = || {
                    args.iter()
                        .map(|a| {
                            let text = format!("{a:?}");
                            text.chars().take(60).collect::<String>()
                        })
                        .collect::<Vec<_>>()
                        .join(", ")
                };
                if outcome.is_err() {
                    failures.push(format!("{}({}) panicked", spec.name, shown()));
                } else if took > slow {
                    failures.push(format!("{}({}) took {took:?}", spec.name, shown()));
                }
            }
        }
    }
    clear_eval_error();
    std::panic::set_hook(hook);

    failures.sort();
    failures.dedup();
    assert!(
        failures.is_empty(),
        "{} of {calls} calls failed:\n{}",
        failures.len(),
        failures
            .iter()
            .take(200)
            .cloned()
            .collect::<Vec<_>>()
            .join("\n")
    );
}

#[test]
fn no_operator_panics_on_extreme_operands() {
    use lora_ast::{BinaryOp, UnaryOp};

    use super::super::binops::{eval_binary, eval_comparison, eval_in, eval_unary, value_eq};

    let ops = [
        BinaryOp::Or,
        BinaryOp::Xor,
        BinaryOp::And,
        BinaryOp::Eq,
        BinaryOp::Ne,
        BinaryOp::Lt,
        BinaryOp::Gt,
        BinaryOp::Le,
        BinaryOp::Ge,
        BinaryOp::Add,
        BinaryOp::Sub,
        BinaryOp::Mul,
        BinaryOp::Div,
        BinaryOp::Mod,
        BinaryOp::Pow,
        BinaryOp::StartsWith,
        BinaryOp::EndsWith,
        BinaryOp::Contains,
        BinaryOp::RegexMatch,
    ];
    let values = extremes();
    let hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(|_| {}));
    let mut failures = Vec::new();
    for a in &values {
        for op in [UnaryOp::Not, UnaryOp::Pos, UnaryOp::Neg] {
            if catch_unwind(AssertUnwindSafe(|| eval_unary(op, a.clone()))).is_err() {
                failures.push(format!("{op:?} {a:.60?}"));
            }
        }
        for b in &values {
            let outcome = catch_unwind(AssertUnwindSafe(|| {
                for op in &ops {
                    eval_binary(op, a.clone(), b.clone());
                    eval_comparison(op, a, b);
                }
                eval_in(a, b);
                value_eq(a, b);
            }));
            if outcome.is_err() {
                failures.push(format!(
                    "{:.60} op {:.60}",
                    format!("{a:?}"),
                    format!("{b:?}")
                ));
            }
        }
    }
    clear_eval_error();
    std::panic::set_hook(hook);
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
