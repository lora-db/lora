use super::state::{Analyzer, PatternContext};
use crate::{errors::*, resolved::*, symbols::*};
use lora_ast::{Expr, MapProjectionSelector, Pattern, PatternPart};
use lora_store::GraphCatalog;
use std::collections::{BTreeMap, BTreeSet};

impl<'a, S: GraphCatalog + ?Sized> Analyzer<'a, S> {
    fn with_child_scope<T>(
        &mut self,
        f: impl FnOnce(&mut Self) -> Result<T, SemanticError>,
    ) -> Result<T, SemanticError> {
        self.scopes.push();
        let result = f(self);
        self.scopes.pop();
        result
    }

    /// Analyze an expression, but allow resolution of projection aliases
    /// (used for ORDER BY which can reference aliases from RETURN/WITH).
    pub(super) fn analyze_expr_with_aliases(
        &mut self,
        expr: &Expr,
        aliases: &BTreeMap<String, VarId>,
    ) -> Result<ResolvedExpr, SemanticError> {
        match expr {
            Expr::Variable(v) => {
                // First try normal scope resolution; only fall back to
                // projection aliases when the variable is not in scope.
                if self.scopes.resolve(&v.name).is_some() {
                    return self.analyze_expr(expr);
                }
                if let Some(&id) = aliases.get(&v.name) {
                    return Ok(ResolvedExpr::Variable(id));
                }
                self.analyze_expr(expr)
            }
            // For property access like `alias.prop`, check if the base is an alias.
            Expr::Property {
                expr: inner,
                key,
                span,
            } => {
                let inner = self.analyze_expr_with_aliases(inner, aliases)?;
                if self.property_access_allowed(&inner, key) {
                    Ok(ResolvedExpr::Property {
                        expr: Box::new(inner),
                        property: key.clone(),
                    })
                } else {
                    Err(SemanticError::UnknownPropertyAt(
                        key.clone(),
                        span.start,
                        span.end,
                    ))
                }
            }
            // For function calls in ORDER BY (e.g. ORDER BY count(p))
            Expr::FunctionCall {
                name,
                distinct,
                args,
                span,
            } => {
                let fn_name = name.join(".");
                validate_function_name(&fn_name, span.start, span.end)?;
                validate_function_arity(&fn_name, args.len())?;

                let args = args
                    .iter()
                    .enumerate()
                    .map(|(idx, a)| {
                        if let Some(lit) = try_vector_enum_literal(&fn_name, idx, a) {
                            Ok(lit)
                        } else {
                            self.analyze_expr_with_aliases(a, aliases)
                        }
                    })
                    .collect::<Result<Vec<_>, _>>()?;

                Ok(ResolvedExpr::Function {
                    name: fn_name,
                    distinct: *distinct,
                    args,
                })
            }
            _ => self.analyze_expr(expr),
        }
    }

    pub(super) fn analyze_expr(&mut self, expr: &Expr) -> Result<ResolvedExpr, SemanticError> {
        match expr {
            Expr::Variable(v) => {
                let id = self.resolve_required_variable(&v.name)?;
                Ok(ResolvedExpr::Variable(id))
            }

            Expr::Integer(v, _) => Ok(ResolvedExpr::Literal(LiteralValue::Integer(*v))),
            Expr::Float(v, _) => Ok(ResolvedExpr::Literal(LiteralValue::Float(*v))),
            Expr::String(v, _) => Ok(ResolvedExpr::Literal(LiteralValue::String(v.clone()))),
            Expr::Bool(v, _) => Ok(ResolvedExpr::Literal(LiteralValue::Bool(*v))),
            Expr::Null(_) => Ok(ResolvedExpr::Literal(LiteralValue::Null)),
            Expr::Parameter(name, _) => Ok(ResolvedExpr::Parameter(name.clone())),

            Expr::List(items, _) => {
                let items = items
                    .iter()
                    .map(|e| self.analyze_expr(e))
                    .collect::<Result<Vec<_>, _>>()?;
                Ok(ResolvedExpr::List(items))
            }

            Expr::Map(items, _) => {
                let mut seen = BTreeSet::new();
                let mut out = Vec::with_capacity(items.len());

                for (k, v) in items {
                    if !seen.insert(k.clone()) {
                        return Err(SemanticError::DuplicateMapKey(k.clone()));
                    }
                    out.push((k.clone(), self.analyze_expr(v)?));
                }

                Ok(ResolvedExpr::Map(out))
            }

            Expr::Property { expr, key, span } => {
                let inner = self.analyze_expr(expr)?;

                if self.property_access_allowed(&inner, key) {
                    Ok(ResolvedExpr::Property {
                        expr: Box::new(inner),
                        property: key.clone(),
                    })
                } else {
                    Err(SemanticError::UnknownPropertyAt(
                        key.clone(),
                        span.start,
                        span.end,
                    ))
                }
            }

            Expr::Binary { lhs, op, rhs, .. } => {
                let lhs = self.analyze_expr(lhs)?;
                let rhs = self.analyze_expr(rhs)?;

                Ok(ResolvedExpr::Binary {
                    lhs: Box::new(lhs),
                    op: *op,
                    rhs: Box::new(rhs),
                })
            }

            Expr::Unary { op, expr, .. } => {
                let expr = self.analyze_expr(expr)?;
                Ok(ResolvedExpr::Unary {
                    op: *op,
                    expr: Box::new(expr),
                })
            }

            Expr::FunctionCall {
                name,
                distinct,
                args,
                span,
                ..
            } => {
                let fn_name = name.join(".");
                validate_function_name(&fn_name, span.start, span.end)?;
                validate_function_arity(&fn_name, args.len())?;

                let args = args
                    .iter()
                    .enumerate()
                    .map(|(idx, a)| {
                        if let Some(lit) = try_vector_enum_literal(&fn_name, idx, a) {
                            Ok(lit)
                        } else {
                            self.analyze_expr(a)
                        }
                    })
                    .collect::<Result<Vec<_>, _>>()?;

                Ok(ResolvedExpr::Function {
                    name: fn_name,
                    distinct: *distinct,
                    args,
                })
            }

            Expr::ListPredicate {
                kind,
                variable,
                list,
                predicate,
                ..
            } => {
                let list = self.analyze_expr(list)?;
                let var_id = self.symbols.new_var();
                let predicate = self.with_child_scope(|this| {
                    this.scopes.declare(variable.name.clone(), var_id);
                    this.analyze_expr(predicate)
                })?;

                Ok(ResolvedExpr::ListPredicate {
                    kind: *kind,
                    variable: var_id,
                    list: Box::new(list),
                    predicate: Box::new(predicate),
                })
            }

            Expr::ListComprehension {
                variable,
                list,
                filter,
                map_expr,
                ..
            } => {
                let list = self.analyze_expr(list)?;
                let var_id = self.symbols.new_var();
                let (filter, map_expr) = self.with_child_scope(|this| {
                    this.scopes.declare(variable.name.clone(), var_id);
                    let filter = filter.as_ref().map(|e| this.analyze_expr(e)).transpose()?;
                    let map_expr = map_expr
                        .as_ref()
                        .map(|e| this.analyze_expr(e))
                        .transpose()?;
                    Ok((filter, map_expr))
                })?;

                Ok(ResolvedExpr::ListComprehension {
                    variable: var_id,
                    list: Box::new(list),
                    filter: filter.map(Box::new),
                    map_expr: map_expr.map(Box::new),
                })
            }

            Expr::Reduce {
                accumulator,
                init,
                variable,
                list,
                expr,
                ..
            } => {
                let init = self.analyze_expr(init)?;
                let list = self.analyze_expr(list)?;
                let acc_id = self.symbols.new_var();
                let var_id = self.symbols.new_var();
                let expr = self.with_child_scope(|this| {
                    this.scopes.declare(accumulator.name.clone(), acc_id);
                    this.scopes.declare(variable.name.clone(), var_id);
                    this.analyze_expr(expr)
                })?;

                Ok(ResolvedExpr::Reduce {
                    accumulator: acc_id,
                    init: Box::new(init),
                    variable: var_id,
                    list: Box::new(list),
                    expr: Box::new(expr),
                })
            }

            Expr::Index {
                expr: inner, index, ..
            } => {
                let expr = self.analyze_expr(inner)?;
                let index = self.analyze_expr(index)?;
                Ok(ResolvedExpr::Index {
                    expr: Box::new(expr),
                    index: Box::new(index),
                })
            }

            Expr::Slice {
                expr: inner,
                from,
                to,
                ..
            } => {
                let expr = self.analyze_expr(inner)?;
                let from = from
                    .as_ref()
                    .map(|e| self.analyze_expr(e))
                    .transpose()?
                    .map(Box::new);
                let to = to
                    .as_ref()
                    .map(|e| self.analyze_expr(e))
                    .transpose()?
                    .map(Box::new);
                Ok(ResolvedExpr::Slice {
                    expr: Box::new(expr),
                    from,
                    to,
                })
            }

            Expr::MapProjection {
                base, selectors, ..
            } => {
                let base = self.analyze_expr(base)?;
                let mut resolved_selectors = Vec::new();
                for sel in selectors {
                    match sel {
                        MapProjectionSelector::Property(name) => {
                            resolved_selectors.push(ResolvedMapSelector::Property(name.clone()));
                        }
                        MapProjectionSelector::AllProperties => {
                            resolved_selectors.push(ResolvedMapSelector::AllProperties);
                        }
                        MapProjectionSelector::Literal(key, expr) => {
                            let resolved = self.analyze_expr(expr)?;
                            resolved_selectors
                                .push(ResolvedMapSelector::Literal(key.clone(), resolved));
                        }
                    }
                }
                Ok(ResolvedExpr::MapProjection {
                    base: Box::new(base),
                    selectors: resolved_selectors,
                })
            }

            Expr::Case {
                input,
                alternatives,
                else_expr,
                ..
            } => {
                let input = input
                    .as_ref()
                    .map(|e| self.analyze_expr(e))
                    .transpose()?
                    .map(Box::new);

                let alternatives = alternatives
                    .iter()
                    .map(|(when, then)| Ok((self.analyze_expr(when)?, self.analyze_expr(then)?)))
                    .collect::<Result<Vec<_>, SemanticError>>()?;

                let else_expr = else_expr
                    .as_ref()
                    .map(|e| self.analyze_expr(e))
                    .transpose()?
                    .map(Box::new);

                Ok(ResolvedExpr::Case {
                    input,
                    alternatives,
                    else_expr,
                })
            }

            Expr::ExistsSubquery {
                pattern, where_, ..
            } => {
                let resolved_pattern =
                    self.analyze_pattern(pattern, PatternContext::OptionalRead)?;
                let resolved_where = where_.as_ref().map(|e| self.analyze_expr(e)).transpose()?;
                Ok(ResolvedExpr::ExistsSubquery {
                    pattern: resolved_pattern,
                    where_: resolved_where.map(Box::new),
                })
            }

            Expr::PatternComprehension {
                pattern: pat_element,
                where_,
                map_expr,
                ..
            } => {
                // Wrap pattern_element in a PatternPart/Pattern for analysis
                let pat = Pattern {
                    parts: vec![PatternPart {
                        binding: None,
                        element: (**pat_element).clone(),
                        span: map_expr.span(),
                    }],
                    span: map_expr.span(),
                };
                let resolved_pattern = self.analyze_pattern(&pat, PatternContext::OptionalRead)?;
                let resolved_where = where_.as_ref().map(|e| self.analyze_expr(e)).transpose()?;
                let resolved_map = self.analyze_expr(map_expr)?;
                Ok(ResolvedExpr::PatternComprehension {
                    pattern: resolved_pattern,
                    where_: resolved_where.map(Box::new),
                    map_expr: Box::new(resolved_map),
                })
            }
        }
    }
}

const AGGREGATE_FUNCTIONS: &[&str] = &[
    "count",
    "sum",
    "avg",
    "min",
    "max",
    "collect",
    "stdev",
    "stdevp",
    "percentilecont",
    "percentiledisc",
];

/// Returns (min_args, max_args) for known functions. `None` means no upper bound (variadic).
fn function_arity(name: &str) -> Option<(usize, Option<usize>)> {
    match name {
        // Aggregate — all take exactly 1 argument (count can take 0 for count(*))
        "count" => Some((0, Some(1))),
        "sum" | "avg" | "min" | "max" | "collect" | "stdev" | "stdevp" => Some((1, Some(1))),
        "percentilecont" | "percentiledisc" => Some((2, Some(2))),
        // Entity introspection — exactly 1
        "id" | "type" | "labels" | "keys" | "properties" | "nodes" | "relationships" => {
            Some((1, Some(1)))
        }
        // String — 1 arg
        "tolower" | "toupper" | "trim" | "ltrim" | "rtrim" | "reverse" => Some((1, Some(1))),
        // String — 2 args
        "split" | "left" | "right" => Some((2, Some(2))),
        // String — 3 args
        "replace" => Some((3, Some(3))),
        // substring: 2 or 3 args
        "substring" => Some((2, Some(3))),
        // Type conversion — exactly 1
        "tostring" | "tointeger" | "toint" | "tofloat" | "toboolean" | "tobooleanornull"
        | "valuetype" => Some((1, Some(1))),
        // String — lpad/rpad take 3
        "lpad" | "rpad" => Some((3, Some(3))),
        // String — char_length/normalize take 1
        "char_length" | "normalize" => Some((1, Some(1))),
        // Math — exactly 1
        "abs" | "ceil" | "floor" | "round" | "sqrt" | "sign" => Some((1, Some(1))),
        // Math — trig / logarithmic (1 arg)
        "log" | "ln" | "log10" | "exp" | "sin" | "cos" | "tan" | "asin" | "acos" | "atan"
        | "degrees" | "radians" => Some((1, Some(1))),
        // Math — atan2 (2 args)
        "atan2" => Some((2, Some(2))),
        // Math — constants (0 args)
        "pi" | "e" | "rand" => Some((0, Some(0))),
        // List / size
        "size" | "length" | "head" | "tail" | "last" => Some((1, Some(1))),
        // range: 2 or 3
        "range" => Some((2, Some(3))),
        // coalesce: 1+
        "coalesce" => Some((1, None)),
        // timestamp: 0
        "timestamp" => Some((0, Some(0))),
        // Temporal constructors: 0 or 1
        "date" | "datetime" | "time" | "localtime" | "localdatetime" => Some((0, Some(1))),
        // duration: exactly 1
        "duration" => Some((1, Some(1))),
        // Temporal namespace functions: exactly 2
        "date.truncate" | "datetime.truncate" | "duration.between" | "duration.indays" => {
            Some((2, Some(2)))
        }
        // Spatial
        "point" => Some((1, Some(1))),
        "distance" | "point.distance" => Some((2, Some(2))),
        "point.withinbbox" => Some((3, Some(3))),
        // Vector
        "vector" => Some((3, Some(3))),
        "tointegerlist" | "tofloatlist" => Some((1, Some(1))),
        "vector_dimension_count" => Some((1, Some(1))),
        "vector_norm" => Some((2, Some(2))),
        "vector_distance" => Some((3, Some(3))),
        "vector.similarity.cosine" | "vector.similarity.euclidean" => Some((2, Some(2))),
        _ => None,
    }
}

/// Special-case the literal-enum arguments of vector construction and
/// metric functions. Bare identifiers like `INTEGER` or `COSINE` parse
/// as `Expr::Variable` today; for these specific slots we treat a bare
/// identifier as a string literal rather than resolving it against the
/// scope. Strings are passed through untouched, and any other expression
/// shape falls through to the normal analyzer so runtime type errors
/// still surface cleanly.
fn try_vector_enum_literal(fn_name: &str, arg_idx: usize, expr: &Expr) -> Option<ResolvedExpr> {
    let fn_lower = fn_name.to_ascii_lowercase();
    let takes_enum_here = match fn_lower.as_str() {
        "vector" => arg_idx == 2,
        "vector_distance" => arg_idx == 2,
        "vector_norm" => arg_idx == 1,
        _ => false,
    };
    if !takes_enum_here {
        return None;
    }
    if let Expr::Variable(v) = expr {
        return Some(ResolvedExpr::Literal(LiteralValue::String(v.name.clone())));
    }
    None
}

fn is_aggregate_function(name: &str) -> bool {
    AGGREGATE_FUNCTIONS
        .iter()
        .any(|function| name.eq_ignore_ascii_case(function))
}

fn validate_function_name(name: &str, start: usize, end: usize) -> Result<(), SemanticError> {
    let lower = name.to_ascii_lowercase();
    if function_arity(&lower).is_some() {
        Ok(())
    } else {
        Err(SemanticError::UnknownFunction(name.to_string(), start, end))
    }
}

fn validate_function_arity(name: &str, arg_count: usize) -> Result<(), SemanticError> {
    let lower = name.to_ascii_lowercase();
    if let Some((min, max)) = function_arity(&lower) {
        if arg_count < min {
            let expected = if max == Some(min) {
                format!("{min}")
            } else if let Some(mx) = max {
                format!("{min}..{mx}")
            } else {
                format!("at least {min}")
            };
            return Err(SemanticError::WrongArity(
                name.to_string(),
                expected,
                arg_count,
            ));
        }
        if let Some(mx) = max {
            if arg_count > mx {
                let expected = if mx == min {
                    format!("{min}")
                } else {
                    format!("{min}..{mx}")
                };
                return Err(SemanticError::WrongArity(
                    name.to_string(),
                    expected,
                    arg_count,
                ));
            }
        }
    }
    Ok(())
}

/// Returns true if the resolved expression contains any aggregate function call.
pub(super) fn expr_contains_aggregate(expr: &ResolvedExpr) -> bool {
    match expr {
        ResolvedExpr::Function { name, args, .. } => {
            if is_aggregate_function(name) {
                return true;
            }
            args.iter().any(expr_contains_aggregate)
        }
        ResolvedExpr::Binary { lhs, rhs, .. } => {
            expr_contains_aggregate(lhs) || expr_contains_aggregate(rhs)
        }
        ResolvedExpr::Unary { expr, .. } => expr_contains_aggregate(expr),
        ResolvedExpr::Property { expr, .. } => expr_contains_aggregate(expr),
        ResolvedExpr::List(items) => items.iter().any(expr_contains_aggregate),
        ResolvedExpr::Map(items) => items.iter().any(|(_, v)| expr_contains_aggregate(v)),
        ResolvedExpr::Case {
            input,
            alternatives,
            else_expr,
        } => {
            input.as_ref().is_some_and(|e| expr_contains_aggregate(e))
                || alternatives
                    .iter()
                    .any(|(w, t)| expr_contains_aggregate(w) || expr_contains_aggregate(t))
                || else_expr
                    .as_ref()
                    .is_some_and(|e| expr_contains_aggregate(e))
        }
        ResolvedExpr::ListPredicate {
            list, predicate, ..
        } => expr_contains_aggregate(list) || expr_contains_aggregate(predicate),
        ResolvedExpr::ListComprehension {
            list,
            filter,
            map_expr,
            ..
        } => {
            expr_contains_aggregate(list)
                || filter.as_ref().is_some_and(|e| expr_contains_aggregate(e))
                || map_expr
                    .as_ref()
                    .is_some_and(|e| expr_contains_aggregate(e))
        }
        ResolvedExpr::Reduce {
            init, list, expr, ..
        } => {
            expr_contains_aggregate(init)
                || expr_contains_aggregate(list)
                || expr_contains_aggregate(expr)
        }
        ResolvedExpr::Index { expr, index } => {
            expr_contains_aggregate(expr) || expr_contains_aggregate(index)
        }
        ResolvedExpr::Slice { expr, from, to } => {
            expr_contains_aggregate(expr)
                || from.as_ref().is_some_and(|e| expr_contains_aggregate(e))
                || to.as_ref().is_some_and(|e| expr_contains_aggregate(e))
        }
        ResolvedExpr::MapProjection { base, selectors } => expr_contains_aggregate(base)
            || selectors.iter().any(
                |s| matches!(s, ResolvedMapSelector::Literal(_, e) if expr_contains_aggregate(e)),
            ),
        ResolvedExpr::ExistsSubquery { .. } | ResolvedExpr::PatternComprehension { .. } => false,
        ResolvedExpr::Variable(_) | ResolvedExpr::Literal(_) | ResolvedExpr::Parameter(_) => false,
    }
}
