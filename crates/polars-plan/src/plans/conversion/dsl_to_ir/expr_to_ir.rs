use super::functions::convert_functions;
use super::*;
use crate::plans::iterator::ArenaExprIter;

pub fn to_expr_ir(expr: Expr, arena: &mut Arena<AExpr>, schema: &Schema) -> PolarsResult<ExprIR> {
    let mut ctx = ExprToIRContext {
        with_fields: None,
        arena,
        schema,
    };
    to_expr_ir_with_context(expr, &mut ctx)
}

pub fn to_expr_ir_with_context(expr: Expr, ctx: &mut ExprToIRContext) -> PolarsResult<ExprIR> {
    let (node, output_name) = to_aexpr_impl(expr, ctx)?;
    Ok(ExprIR::new(node, OutputName::Alias(output_name)))
}

pub fn to_expr_ir_materialized_lit(
    expr: Expr,
    arena: &mut Arena<AExpr>,
    schema: &Schema,
) -> PolarsResult<ExprIR> {
    let mut ctx = ExprToIRContext {
        with_fields: None,
        arena,
        schema,
    };
    let (node, output_name) = to_aexpr_impl_materialized_lit(expr, &mut ctx)?;
    Ok(ExprIR::new(node, OutputName::Alias(output_name)))
}

pub(super) fn to_expr_irs(
    input: Vec<Expr>,
    arena: &mut Arena<AExpr>,
    schema: &Schema,
) -> PolarsResult<Vec<ExprIR>> {
    input
        .into_iter()
        .map(|e| to_expr_ir(e, arena, schema))
        .collect()
}

pub(super) fn to_expr_irs_with_context(
    input: Vec<Expr>,
    ctx: &mut ExprToIRContext,
) -> PolarsResult<Vec<ExprIR>> {
    input
        .into_iter()
        .map(|e| to_expr_ir_with_context(e, ctx))
        .collect()
}

fn to_aexpr_impl_materialized_lit(
    expr: Expr,
    ctx: &mut ExprToIRContext,
) -> PolarsResult<(Node, PlSmallStr)> {
    // Already convert `Lit Float and Lit Int` expressions that are not used in a binary / function expression.
    // This means they can be materialized immediately
    let e = match expr {
        Expr::Literal(lv @ LiteralValue::Dyn(_)) => Expr::Literal(lv.materialize()),
        Expr::Alias(inner, name) if matches!(&*inner, Expr::Literal(LiteralValue::Dyn(_))) => {
            let Expr::Literal(lv) = &*inner else {
                unreachable!()
            };
            Expr::Alias(Arc::new(Expr::Literal(lv.clone().materialize())), name)
        },
        e => e,
    };
    to_aexpr_impl(e, ctx)
}

pub struct ExprToIRContext<'a> {
    pub with_fields: Option<(Node, Schema)>,
    pub arena: &'a mut Arena<AExpr>,
    pub schema: &'a Schema,
}

/// Converts expression to AExpr and adds it to the arena, which uses an arena (Vec) for allocation.
pub(super) fn to_aexpr_impl(
    expr: Expr,
    ctx: &mut ExprToIRContext,
) -> PolarsResult<(Node, PlSmallStr)> {
    let owned = Arc::unwrap_or_clone;
    let (v, output_name) = match expr {
        Expr::Explode { input, skip_empty } => {
            let (expr, output_name) = to_aexpr_impl(owned(input), ctx)?;
            (AExpr::Explode { expr, skip_empty }, output_name)
        },
        Expr::Alias(e, name) => return Ok((to_aexpr_impl(owned(e), ctx)?.0, name)),
        Expr::Literal(lv) => {
            let output_name = lv.output_column_name().clone();
            (AExpr::Literal(lv), output_name)
        },
        Expr::Column(name) => (AExpr::Column(name.clone()), name),
        Expr::BinaryExpr { left, op, right } => {
            let (l, output_name) = to_aexpr_impl(owned(left), ctx)?;
            let (r, _) = to_aexpr_impl(owned(right), ctx)?;
            (
                AExpr::BinaryExpr {
                    left: l,
                    op,
                    right: r,
                },
                output_name,
            )
        },
        Expr::Cast {
            expr,
            dtype,
            options,
        } => {
            let (expr, output_name) = to_aexpr_impl(owned(expr), ctx)?;
            (
                AExpr::Cast {
                    expr,
                    dtype: dtype.into_datatype(ctx.schema)?,
                    options,
                },
                output_name,
            )
        },
        Expr::Gather {
            expr,
            idx,
            returns_scalar,
        } => {
            let (expr, output_name) = to_aexpr_impl(owned(expr), ctx)?;
            let (idx, _) = to_aexpr_impl_materialized_lit(owned(idx), ctx)?;
            (
                AExpr::Gather {
                    expr,
                    idx,
                    returns_scalar,
                },
                output_name,
            )
        },
        Expr::Sort { expr, options } => {
            let (expr, output_name) = to_aexpr_impl(owned(expr), ctx)?;
            (AExpr::Sort { expr, options }, output_name)
        },
        Expr::SortBy {
            expr,
            by,
            sort_options,
        } => {
            let (expr, output_name) = to_aexpr_impl(owned(expr), ctx)?;
            let by = by
                .into_iter()
                .map(|e| Ok(to_aexpr_impl(e, ctx)?.0))
                .collect::<PolarsResult<_>>()?;

            (
                AExpr::SortBy {
                    expr,
                    by,
                    sort_options,
                },
                output_name,
            )
        },
        Expr::Filter { input, by } => {
            let (input, output_name) = to_aexpr_impl(owned(input), ctx)?;
            let (by, _) = to_aexpr_impl(owned(by), ctx)?;
            (AExpr::Filter { input, by }, output_name)
        },
        Expr::Agg(agg) => {
            let (a_agg, output_name) = match agg {
                AggExpr::Min {
                    input,
                    propagate_nans,
                } => {
                    let (input, output_name) = to_aexpr_impl_materialized_lit(owned(input), ctx)?;
                    (
                        IRAggExpr::Min {
                            input,
                            propagate_nans,
                        },
                        output_name,
                    )
                },
                AggExpr::Max {
                    input,
                    propagate_nans,
                } => {
                    let (input, output_name) = to_aexpr_impl_materialized_lit(owned(input), ctx)?;
                    (
                        IRAggExpr::Max {
                            input,
                            propagate_nans,
                        },
                        output_name,
                    )
                },
                AggExpr::Median(input) => {
                    let (input, output_name) = to_aexpr_impl_materialized_lit(owned(input), ctx)?;
                    (IRAggExpr::Median(input), output_name)
                },
                AggExpr::NUnique(input) => {
                    let (input, output_name) = to_aexpr_impl_materialized_lit(owned(input), ctx)?;
                    (IRAggExpr::NUnique(input), output_name)
                },
                AggExpr::First(input) => {
                    let (input, output_name) = to_aexpr_impl_materialized_lit(owned(input), ctx)?;
                    (IRAggExpr::First(input), output_name)
                },
                AggExpr::Last(input) => {
                    let (input, output_name) = to_aexpr_impl_materialized_lit(owned(input), ctx)?;
                    (IRAggExpr::Last(input), output_name)
                },
                AggExpr::Mean(input) => {
                    let (input, output_name) = to_aexpr_impl_materialized_lit(owned(input), ctx)?;
                    (IRAggExpr::Mean(input), output_name)
                },
                AggExpr::Implode(input) => {
                    let (input, output_name) = to_aexpr_impl_materialized_lit(owned(input), ctx)?;
                    (IRAggExpr::Implode(input), output_name)
                },
                AggExpr::Count(input, include_nulls) => {
                    let (input, output_name) = to_aexpr_impl_materialized_lit(owned(input), ctx)?;
                    (IRAggExpr::Count(input, include_nulls), output_name)
                },
                AggExpr::Quantile {
                    expr,
                    quantile,
                    method,
                } => {
                    let (expr, output_name) = to_aexpr_impl_materialized_lit(owned(expr), ctx)?;
                    let (quantile, _) = to_aexpr_impl_materialized_lit(owned(quantile), ctx)?;
                    (
                        IRAggExpr::Quantile {
                            expr,
                            quantile,
                            method,
                        },
                        output_name,
                    )
                },
                AggExpr::Sum(input) => {
                    let (input, output_name) = to_aexpr_impl_materialized_lit(owned(input), ctx)?;
                    (IRAggExpr::Sum(input), output_name)
                },
                AggExpr::Std(input, ddof) => {
                    let (input, output_name) = to_aexpr_impl_materialized_lit(owned(input), ctx)?;
                    (IRAggExpr::Std(input, ddof), output_name)
                },
                AggExpr::Var(input, ddof) => {
                    let (input, output_name) = to_aexpr_impl_materialized_lit(owned(input), ctx)?;
                    (IRAggExpr::Var(input, ddof), output_name)
                },
                AggExpr::AggGroups(input) => {
                    let (input, output_name) = to_aexpr_impl_materialized_lit(owned(input), ctx)?;
                    (IRAggExpr::AggGroups(input), output_name)
                },
            };
            (AExpr::Agg(a_agg), output_name)
        },
        Expr::Ternary {
            predicate,
            truthy,
            falsy,
        } => {
            let (p, _) = to_aexpr_impl_materialized_lit(owned(predicate), ctx)?;
            let (t, output_name) = to_aexpr_impl(owned(truthy), ctx)?;
            let (f, _) = to_aexpr_impl(owned(falsy), ctx)?;
            (
                AExpr::Ternary {
                    predicate: p,
                    truthy: t,
                    falsy: f,
                },
                output_name,
            )
        },
        Expr::AnonymousFunction {
            input,
            function,
            output_type,
            options,
            fmt_str,
        } => {
            let e = to_expr_irs_with_context(input, ctx)?;
            let output_name = if e.is_empty() {
                fmt_str.as_ref().clone()
            } else {
                e[0].output_name().clone()
            };

            let function = function.materialize()?;
            let output_type = output_type.materialize()?;
            function.as_ref().resolve_dsl(ctx.schema)?;
            output_type.as_ref().resolve_dsl(ctx.schema)?;

            (
                AExpr::AnonymousFunction {
                    input: e,
                    function: LazySerde::Deserialized(function),
                    output_type: LazySerde::Deserialized(output_type),
                    options,
                    fmt_str,
                },
                output_name,
            )
        },
        Expr::Function { input, function } => {
            return convert_functions(input, function, ctx);
        },
        Expr::Window {
            function,
            partition_by,
            order_by,
            options,
        } => {
            let (function, output_name) = to_aexpr_impl(owned(function), ctx)?;
            let order_by = if let Some((e, options)) = order_by {
                Some((to_aexpr_impl(owned(e.clone()), ctx)?.0, options))
            } else {
                None
            };

            (
                AExpr::Window {
                    function,
                    partition_by: partition_by
                        .into_iter()
                        .map(|e| Ok(to_aexpr_impl_materialized_lit(e, ctx)?.0))
                        .collect::<PolarsResult<_>>()?,
                    order_by,
                    options,
                },
                output_name,
            )
        },
        Expr::Slice {
            input,
            offset,
            length,
        } => {
            let (input, output_name) = to_aexpr_impl(owned(input), ctx)?;
            let (offset, _) = to_aexpr_impl_materialized_lit(owned(offset), ctx)?;
            let (length, _) = to_aexpr_impl_materialized_lit(owned(length), ctx)?;
            (
                AExpr::Slice {
                    input,
                    offset,
                    length,
                },
                output_name,
            )
        },
        Expr::Eval {
            expr,
            evaluation,
            variant,
        } => {
            let (expr, output_name) = to_aexpr_impl(owned(expr), ctx)?;
            let expr_dtype =
                ctx.arena
                    .get(expr)
                    .to_dtype(ctx.schema, Context::Default, ctx.arena)?;
            let element_dtype = variant.element_dtype(&expr_dtype)?;
            let evaluation_schema = Schema::from_iter([(PlSmallStr::EMPTY, element_dtype.clone())]);
            let mut evaluation_ctx = ExprToIRContext {
                with_fields: None,
                schema: &evaluation_schema,
                arena: ctx.arena,
            };
            let (evaluation, _) = to_aexpr_impl(owned(evaluation), &mut evaluation_ctx)?;

            match variant {
                EvalVariant::List => {
                    for (_, e) in ArenaExprIter::iter(&&*ctx.arena, evaluation) {
                        if let AExpr::Column(name) = e {
                            polars_ensure!(
                                name.is_empty(),
                                ComputeError:
                                "named columns are not allowed in `list.eval`; consider using `element` or `col(\"\")`"
                            );
                        }
                    }
                },
                EvalVariant::Cumulative { .. } => {
                    polars_ensure!(
                        is_scalar_ae(evaluation, ctx.arena),
                        InvalidOperation: "`cumulative_eval` is not allowed with non-scalar output"
                    )
                },
            }

            (
                AExpr::Eval {
                    expr,
                    evaluation,
                    variant,
                },
                output_name,
            )
        },
        Expr::Len => (AExpr::Len, get_len_name()),
        Expr::KeepName(expr) => {
            let (expr, _) = to_aexpr_impl(owned(expr), ctx)?;
            let name = ArenaExprIter::iter(&&*ctx.arena, expr).find_map(|e| match e.1 {
                AExpr::Column(name) => Some(name.clone()),
                #[cfg(feature = "dtype-struct")]
                AExpr::Function {
                    input: _,
                    function: IRFunctionExpr::StructExpr(IRStructFunction::FieldByName(name)),
                    options: _,
                } => Some(name.clone()),
                _ => None,
            });
            let Some(name) = name else {
                polars_bail!(
                    InvalidOperation:
                    "`name.keep_name` expected at least one column or struct.field"
                );
            };
            return Ok((expr, name));
        },
        Expr::RenameAlias { expr, function } => {
            let (expr, name) = to_aexpr_impl(owned(expr), ctx)?;
            let name = function.call(&name)?;
            return Ok((expr, name));
        },
        #[cfg(feature = "dtype-struct")]
        Expr::Field(name) => {
            assert_eq!(
                name.len(),
                1,
                "should have been handled in expression expansion"
            );
            let name = &name[0];

            let Some((input, with_fields)) = &ctx.with_fields else {
                polars_bail!(InvalidOperation: "`pl.field()` called outside of struct context");
            };

            if !with_fields.contains(name) {
                polars_bail!(
                    InvalidOperation: "field `{name}` does not exist on struct with fields {:?}",
                    with_fields.iter_names_cloned().collect::<Vec<_>>().as_slice()
                );
            }

            let function = IRFunctionExpr::StructExpr(IRStructFunction::FieldByName(name.clone()));
            let options = function.function_options();
            (
                AExpr::Function {
                    input: vec![ExprIR::new(*input, OutputName::Alias(PlSmallStr::EMPTY))],
                    function,
                    options,
                },
                name.clone(),
            )
        },

        e @ Expr::SubPlan { .. } | e @ Expr::Selector(_) => {
            polars_bail!(InvalidOperation: "'Expr: {}' not allowed in this context/location", e)
        },
    };
    Ok((ctx.arena.add(v), output_name))
}
