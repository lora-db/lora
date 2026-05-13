use super::state::{Analyzer, PatternContext};
use crate::analyzer::FunctionId;
use crate::{errors::*, resolved::*, symbols::*};
use lora_ast::{Expr, LiteralTypeExpr, MapProjectionSelector, Pattern, PatternPart};
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
                let legacy_cast = legacy_cast_target(&fn_name);
                let resolve_name = legacy_cast.map_or(fn_name.as_str(), |(name, _)| name);
                let function = resolve_function_name(resolve_name, span.start, span.end)?;
                if legacy_cast.is_some() {
                    validate_fixed_arity(&fn_name, args.len(), 1)?;
                } else {
                    validate_function_arity(function, &fn_name, args.len())?;
                }

                let mut args = args
                    .iter()
                    .enumerate()
                    .map(|(idx, a)| {
                        if let Some(lit) = try_builtin_literal(resolve_name, idx, a) {
                            Ok(lit)
                        } else {
                            self.analyze_expr_with_aliases(a, aliases)
                        }
                    })
                    .collect::<Result<Vec<_>, _>>()?;
                if let Some((_, target)) = legacy_cast {
                    args.push(ResolvedExpr::Literal(LiteralValue::TypeName(
                        target.to_string(),
                    )));
                }

                Ok(ResolvedExpr::Function {
                    function,
                    distinct: *distinct,
                    args,
                })
            }
            Expr::TypeCast {
                expr,
                target,
                try_cast,
                span,
            } => {
                let expr = self.analyze_expr_with_aliases(expr, aliases)?;
                lower_type_cast_expr(expr, target, *try_cast, span.start, span.end)
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
                let legacy_cast = legacy_cast_target(&fn_name);
                let resolve_name = legacy_cast.map_or(fn_name.as_str(), |(name, _)| name);
                let function = resolve_function_name(resolve_name, span.start, span.end)?;
                if legacy_cast.is_some() {
                    validate_fixed_arity(&fn_name, args.len(), 1)?;
                } else {
                    validate_function_arity(function, &fn_name, args.len())?;
                }

                let mut args = args
                    .iter()
                    .enumerate()
                    .map(|(idx, a)| {
                        if let Some(lit) = try_builtin_literal(resolve_name, idx, a) {
                            Ok(lit)
                        } else {
                            self.analyze_expr(a)
                        }
                    })
                    .collect::<Result<Vec<_>, _>>()?;
                if let Some((_, target)) = legacy_cast {
                    args.push(ResolvedExpr::Literal(LiteralValue::TypeName(
                        target.to_string(),
                    )));
                }

                Ok(ResolvedExpr::Function {
                    function,
                    distinct: *distinct,
                    args,
                })
            }
            Expr::TypeCast {
                expr,
                target,
                try_cast,
                span,
            } => {
                let expr = self.analyze_expr(expr)?;
                lower_type_cast_expr(expr, target, *try_cast, span.start, span.end)
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

/// Special-case builtin literal slots. Bare identifiers like `INTEGER`
/// or `COSINE` parse as `Expr::Variable` today; for these specific slots
/// we treat them as type or enum literals rather than resolving them
/// against the scope. Strings are passed through untouched, and any
/// other expression shape falls through to the normal analyzer so runtime
/// type errors still surface cleanly.
fn try_builtin_literal(fn_name: &str, arg_idx: usize, expr: &Expr) -> Option<ResolvedExpr> {
    let fn_lower = fn_name.to_ascii_lowercase();
    if let Expr::Variable(v) = expr {
        if super::builtin_signatures::accepts_type_literal(&fn_lower, arg_idx) {
            return Some(ResolvedExpr::Literal(LiteralValue::TypeName(
                v.name.clone(),
            )));
        }
        if super::builtin_signatures::accepts_enum_literal(&fn_lower, arg_idx) {
            return Some(ResolvedExpr::Literal(LiteralValue::String(v.name.clone())));
        }
    }
    None
}

fn lower_type_cast_expr(
    expr: ResolvedExpr,
    target: &LiteralTypeExpr,
    try_cast: bool,
    start: usize,
    end: usize,
) -> Result<ResolvedExpr, SemanticError> {
    let (name, args) = type_cast_lowering(expr, target, try_cast);
    let function = resolve_function_name(&name, start, end)?;
    Ok(ResolvedExpr::Function {
        function,
        distinct: false,
        args,
    })
}

fn type_cast_lowering(
    expr: ResolvedExpr,
    target: &LiteralTypeExpr,
    try_cast: bool,
) -> (String, Vec<ResolvedExpr>) {
    let name = if try_cast { "cast.try" } else { "cast.to" };
    (
        name.to_string(),
        vec![
            expr,
            ResolvedExpr::Literal(LiteralValue::TypeName(format_literal_type(target))),
        ],
    )
}

fn normalize_literal_type_name(name: &str) -> String {
    name.trim()
        .chars()
        .map(|ch| match ch {
            '-' => '_',
            ch if ch.is_whitespace() => '_',
            _ => ch.to_ascii_uppercase(),
        })
        .collect()
}

fn format_literal_type(target: &LiteralTypeExpr) -> String {
    match target {
        LiteralTypeExpr::Named { name, .. } => normalize_literal_type_name(name),
        LiteralTypeExpr::List { inner, .. } => format!("LIST<{}>", format_literal_type(inner)),
        LiteralTypeExpr::Vector {
            coordinate,
            dimension,
            ..
        } => format!("VECTOR<{coordinate}>({dimension})"),
    }
}

fn legacy_cast_target(fn_name: &str) -> Option<(&'static str, &'static str)> {
    let lower = fn_name.to_ascii_lowercase();
    Some(match lower.as_str() {
        "tostring" => ("cast.to", "STRING"),
        "tointeger" => ("cast.to", "INTEGER"),
        "tofloat" => ("cast.to", "FLOAT"),
        "toboolean" => ("cast.to", "BOOLEAN"),
        "tostringornull" => ("cast.try", "STRING"),
        "tointegerornull" => ("cast.try", "INTEGER"),
        "tofloatornull" => ("cast.try", "FLOAT"),
        "tobooleanornull" => ("cast.try", "BOOLEAN"),
        _ => return None,
    })
}

fn resolve_function_name(
    name: &str,
    start: usize,
    end: usize,
) -> Result<FunctionId, SemanticError> {
    super::builtin_signatures::resolve_function(name)
        .ok_or_else(|| SemanticError::UnknownFunction(name.to_string(), start, end))
}

fn validate_fixed_arity(
    source_name: &str,
    arg_count: usize,
    expected: usize,
) -> Result<(), SemanticError> {
    if arg_count == expected {
        Ok(())
    } else {
        Err(SemanticError::WrongArity(
            source_name.to_string(),
            expected.to_string(),
            arg_count,
        ))
    }
}

fn validate_function_arity(
    function: FunctionId,
    source_name: &str,
    arg_count: usize,
) -> Result<(), SemanticError> {
    let arity = function.arity();
    let min = arity.min;
    let max = arity.max;
    if arg_count < min {
        let expected = if max == Some(min) {
            format!("{min}")
        } else if let Some(mx) = max {
            format!("{min}..{mx}")
        } else {
            format!("at least {min}")
        };
        return Err(SemanticError::WrongArity(
            source_name.to_string(),
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
                source_name.to_string(),
                expected,
                arg_count,
            ));
        }
    }
    Ok(())
}

/// Returns true if the resolved expression contains any aggregate function call.
pub(super) fn expr_contains_aggregate(expr: &ResolvedExpr) -> bool {
    match expr {
        ResolvedExpr::Function { function, args, .. } => {
            if function.is_aggregate() {
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
