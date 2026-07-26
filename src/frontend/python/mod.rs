//! A deliberately small Python-to-WVM frontend.

mod error;
mod hir;
mod lower;

pub use error::{PythonFrontendError, SourceLocation};

use rustpython_parser::Parse;
use rustpython_parser::ast::{self, CmpOp, Constant, Operator, Ranged, UnaryOp};

use crate::executable::ExecutableFunction;

use self::hir::{HirExpression, HirExpressionKind, HirFunction, HirStatement, HirStatementKind};

/// Compiles one named, zero-argument Python function into WVM bytecode and its
/// WVM-PC-based StructureMap.
pub fn compile_python_function(
    source: &str,
    function_name: &str,
) -> Result<ExecutableFunction, PythonFrontendError> {
    let suite = ast::Suite::parse(source, "<python frontend>").map_err(|error| {
        PythonFrontendError::new(
            format!("Python parse error: {}", error.error),
            Some(location_at(source, u32::from(error.offset) as usize)),
        )
    })?;

    let mut selected = suite.iter().filter_map(|statement| match statement {
        ast::Stmt::FunctionDef(function) if function.name.as_str() == function_name => {
            Some(function)
        }
        _ => None,
    });
    let function = selected.next().ok_or_else(|| {
        PythonFrontendError::new(format!("function `{function_name}` was not found"), None)
    })?;
    if let Some(duplicate) = selected.next() {
        return Err(error_at(
            source,
            duplicate,
            format!("function `{function_name}` is defined more than once"),
        ));
    }

    ensure_supported_function(source, function)?;
    let body = lower_statements(source, &function.body, false)?;
    if !matches!(
        body.last().map(|statement| &statement.kind),
        Some(HirStatementKind::Return(_))
    ) {
        return Err(error_at(
            source,
            function,
            "selected function must end with return",
        ));
    }

    lower::lower(HirFunction { body })
}

fn ensure_supported_function(
    source: &str,
    function: &ast::StmtFunctionDef,
) -> Result<(), PythonFrontendError> {
    let arguments = &function.args;
    if !arguments.posonlyargs.is_empty()
        || !arguments.args.is_empty()
        || arguments.vararg.is_some()
        || !arguments.kwonlyargs.is_empty()
        || arguments.kwarg.is_some()
    {
        return Err(error_at(
            source,
            function,
            "selected function must have zero arguments",
        ));
    }
    if !function.decorator_list.is_empty()
        || function.returns.is_some()
        || !function.type_params.is_empty()
        || function.type_comment.is_some()
    {
        return Err(error_at(
            source,
            function,
            "decorators, annotations, and type comments are unsupported",
        ));
    }
    Ok(())
}

fn lower_statements(
    source: &str,
    statements: &[ast::Stmt],
    in_loop: bool,
) -> Result<Vec<HirStatement>, PythonFrontendError> {
    statements
        .iter()
        .map(|statement| {
            let location = location_of(source, statement);
            let kind = match statement {
                ast::Stmt::Assign(assign) => {
                    if assign.targets.len() != 1 || assign.type_comment.is_some() {
                        return Err(error_at(
                            source,
                            assign,
                            "only a single unannotated assignment target is supported",
                        ));
                    }
                    let ast::Expr::Name(target) = &assign.targets[0] else {
                        return Err(error_at(
                            source,
                            &assign.targets[0],
                            "assignment target must be a local name",
                        ));
                    };
                    HirStatementKind::Assign {
                        name: target.id.to_string(),
                        value: lower_expression(source, &assign.value)?,
                    }
                }
                ast::Stmt::While(while_statement) => {
                    if in_loop {
                        return Err(error_at(
                            source,
                            while_statement,
                            "nested while loops are unsupported",
                        ));
                    }
                    if !while_statement.orelse.is_empty() {
                        return Err(error_at(
                            source,
                            while_statement,
                            "while else is unsupported",
                        ));
                    }
                    HirStatementKind::While {
                        condition: lower_expression(source, &while_statement.test)?,
                        body: lower_statements(source, &while_statement.body, true)?,
                    }
                }
                ast::Stmt::Return(return_statement) if !in_loop => {
                    let value = return_statement.value.as_ref().ok_or_else(|| {
                        error_at(source, return_statement, "return must include a value")
                    })?;
                    HirStatementKind::Return(lower_expression(source, value)?)
                }
                ast::Stmt::Return(return_statement) => {
                    return Err(error_at(
                        source,
                        return_statement,
                        "return inside a while loop is unsupported",
                    ));
                }
                _ => return Err(error_at(source, statement, "unsupported Python statement")),
            };
            Ok(HirStatement { kind, location })
        })
        .collect()
}

fn lower_expression(
    source: &str,
    expression: &ast::Expr,
) -> Result<HirExpression, PythonFrontendError> {
    let location = location_of(source, expression);
    let kind = match expression {
        ast::Expr::Constant(constant) => {
            HirExpressionKind::I64(parse_i64_constant(source, constant, false)?)
        }
        ast::Expr::UnaryOp(unary) if unary.op == UnaryOp::USub => {
            let ast::Expr::Constant(constant) = unary.operand.as_ref() else {
                return Err(error_at(
                    source,
                    unary,
                    "unary minus requires an integer literal",
                ));
            };
            HirExpressionKind::I64(parse_i64_constant(source, constant, true)?)
        }
        ast::Expr::Name(name) => HirExpressionKind::Name(name.id.to_string()),
        ast::Expr::BinOp(binary) if binary.op == Operator::Add => HirExpressionKind::Add(
            Box::new(lower_expression(source, &binary.left)?),
            Box::new(lower_expression(source, &binary.right)?),
        ),
        ast::Expr::Compare(compare)
            if compare.ops.as_slice() == [CmpOp::Lt] && compare.comparators.len() == 1 =>
        {
            HirExpressionKind::SignedLt(
                Box::new(lower_expression(source, &compare.left)?),
                Box::new(lower_expression(source, &compare.comparators[0])?),
            )
        }
        _ => {
            return Err(error_at(
                source,
                expression,
                "unsupported Python expression",
            ));
        }
    };
    Ok(HirExpression { kind, location })
}

fn parse_i64_constant(
    source: &str,
    constant: &ast::ExprConstant,
    negative: bool,
) -> Result<i64, PythonFrontendError> {
    let Constant::Int(value) = &constant.value else {
        return Err(error_at(
            source,
            constant,
            "only i64 integer literals are supported",
        ));
    };
    let digits = value.to_string();
    let literal = if negative {
        format!("-{digits}")
    } else {
        digits
    };
    literal
        .parse()
        .map_err(|_| error_at(source, constant, "integer literal is outside the i64 range"))
}

fn error_at(source: &str, ranged: &impl Ranged, message: impl Into<String>) -> PythonFrontendError {
    PythonFrontendError::new(message, Some(location_of(source, ranged)))
}

fn location_of(source: &str, ranged: &impl Ranged) -> SourceLocation {
    location_at(source, u32::from(ranged.start()) as usize)
}

fn location_at(source: &str, offset: usize) -> SourceLocation {
    let prefix = &source[..offset.min(source.len())];
    SourceLocation {
        line: prefix.bytes().filter(|byte| *byte == b'\n').count() + 1,
        column: prefix
            .rsplit('\n')
            .next()
            .map_or(1, |line| line.chars().count() + 1),
    }
}
