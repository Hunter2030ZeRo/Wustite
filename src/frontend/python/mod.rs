//! A deliberately small Python-to-WVM frontend.

mod error;
mod expression;
mod hir;
mod lower;
mod parameters;
mod range;
mod statements;

use std::collections::{HashMap, HashSet};

pub use error::{PythonFrontendError, SourceLocation};

use rustpython_parser::Parse;
use rustpython_parser::ast::{self, Ranged};

use crate::executable::ExecutableFunction;
use crate::object::ClassObject;

use self::hir::{HirFunction, HirStatementKind};

/// Compiles one named Python function into WVM bytecode and its StructureMap.
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
    Compiler {
        source,
        suite: &suite,
        stack: Vec::new(),
        compiled: HashMap::new(),
        compiled_classes: HashMap::new(),
    }
    .compile_named(function_name, None)
}

struct Compiler<'a> {
    source: &'a str,
    suite: &'a [ast::Stmt],
    stack: Vec<String>,
    compiled: HashMap<String, ExecutableFunction>,
    compiled_classes: HashMap<String, ClassObject>,
}

impl Compiler<'_> {
    fn compile_named(
        &mut self,
        name: &str,
        reference: Option<&ast::ExprName>,
    ) -> Result<ExecutableFunction, PythonFrontendError> {
        if self.stack.iter().any(|active| active == name) {
            let location = reference.map(|value| location_of(self.source, value));
            return Err(PythonFrontendError::new(
                format!("unsupported recursive function reference cycle involving `{name}`"),
                location,
            ));
        }
        if let Some(function) = self.compiled.get(name) {
            return Ok(function.clone());
        }
        let function = self.find_function(name)?.clone();
        ensure_supported_function(self.source, &function)?;
        self.stack.push(name.to_string());
        let result = self.compile_definition(&function, name);
        self.stack.pop();
        if let Ok(function) = &result {
            self.compiled.insert(name.to_string(), function.clone());
        }
        result
    }

    fn compile_definition(
        &mut self,
        function: &ast::StmtFunctionDef,
        current_name: &str,
    ) -> Result<ExecutableFunction, PythonFrontendError> {
        let parameters = parameters::lower_parameters(self.source, function)?;
        let local_names = local_assignment_names(&function.body);
        let mut initialized_names: HashSet<_> = parameters
            .iter()
            .map(|parameter| parameter.name.clone())
            .collect();
        let body = self.lower_statements(
            &function.body,
            statements::FunctionScope {
                current_name,
                local_names: &local_names,
            },
            &mut initialized_names,
        )?;
        if !matches!(
            body.last().map(|statement| &statement.kind),
            Some(HirStatementKind::Return(_))
        ) {
            return Err(error_at(
                self.source,
                function,
                "selected function must end with return",
            ));
        }
        lower::lower(HirFunction { parameters, body }).map(|mut executable| {
            executable.set_name(current_name.to_string());
            executable
        })
    }

    fn find_function(&self, name: &str) -> Result<&ast::StmtFunctionDef, PythonFrontendError> {
        let mut matches = self.suite.iter().filter_map(|statement| match statement {
            ast::Stmt::FunctionDef(function) if function.name.as_str() == name => Some(function),
            _ => None,
        });
        let function = matches.next().ok_or_else(|| {
            PythonFrontendError::new(format!("function `{name}` was not found"), None)
        })?;
        if let Some(duplicate) = matches.next() {
            return Err(error_at(
                self.source,
                duplicate,
                format!("function `{name}` is defined more than once"),
            ));
        }
        Ok(function)
    }

    fn has_function(&self, name: &str) -> bool {
        self.suite.iter().any(|statement| {
            matches!(statement, ast::Stmt::FunctionDef(function) if function.name.as_str() == name)
        })
    }

    fn has_class(&self, name: &str) -> bool {
        self.suite.iter().any(|statement| {
            matches!(statement, ast::Stmt::ClassDef(class) if class.name.as_str() == name)
        })
    }

    fn compile_class(&mut self, name: &str) -> Result<ClassObject, PythonFrontendError> {
        if let Some(class) = self.compiled_classes.get(name) {
            return Ok(class.clone());
        }
        let class = self
            .suite
            .iter()
            .find_map(|statement| match statement {
                ast::Stmt::ClassDef(class) if class.name.as_str() == name => Some(class.clone()),
                _ => None,
            })
            .ok_or_else(|| {
                PythonFrontendError::new(format!("class `{name}` was not found"), None)
            })?;
        if !class.bases.is_empty()
            || !class.keywords.is_empty()
            || !class.decorator_list.is_empty()
            || !class.type_params.is_empty()
        {
            return Err(error_at(
                self.source,
                &class,
                "inheritance, decorators, keywords, and type parameters are unsupported",
            ));
        }
        let mut methods = Vec::new();
        for statement in &class.body {
            let ast::Stmt::FunctionDef(function) = statement else {
                return Err(error_at(
                    self.source,
                    statement,
                    "class bodies may only contain methods",
                ));
            };
            ensure_supported_function(self.source, function)?;
            let qualified = format!("{name}.{}", function.name);
            methods.push((
                function.name.to_string(),
                self.compile_method_definition(function, &qualified)?,
            ));
        }
        let class = ClassObject::new(name.to_string(), methods);
        self.compiled_classes
            .insert(name.to_string(), class.clone());
        Ok(class)
    }

    fn compile_method_definition(
        &mut self,
        function: &ast::StmtFunctionDef,
        qualified_name: &str,
    ) -> Result<ExecutableFunction, PythonFrontendError> {
        let parameters = parameters::lower_method_parameters(self.source, function)?;
        let local_names = local_assignment_names(&function.body);
        let mut initialized_names: HashSet<_> = parameters
            .iter()
            .map(|parameter| parameter.name.clone())
            .collect();
        let mut body = self.lower_statements(
            &function.body,
            statements::FunctionScope {
                current_name: qualified_name,
                local_names: &local_names,
            },
            &mut initialized_names,
        )?;
        if !matches!(
            body.last().map(|statement| &statement.kind),
            Some(HirStatementKind::Return(_))
        ) {
            let location = location_of(self.source, function);
            body.push(self::hir::HirStatement {
                kind: HirStatementKind::Return(self::hir::HirExpression {
                    kind: self::hir::HirExpressionKind::None,
                    location,
                }),
                location,
            });
        }
        lower::lower(HirFunction { parameters, body }).map(|mut executable| {
            executable.set_name(qualified_name.to_string());
            executable
        })
    }

    fn module_constant(&self, name: &str) -> Option<ast::Expr> {
        self.module_constant_expression(name, &mut HashSet::new())
    }

    fn module_constant_expression(
        &self,
        name: &str,
        active: &mut HashSet<String>,
    ) -> Option<ast::Expr> {
        if !active.insert(name.to_string()) {
            return None;
        }
        let value = self.suite.iter().find_map(|statement| {
            let ast::Stmt::Assign(assign) = statement else {
                return None;
            };
            let [ast::Expr::Name(target)] = assign.targets.as_slice() else {
                return None;
            };
            (target.id.as_str() == name).then(|| assign.value.as_ref().clone())
        })?;
        let supported = self.is_module_constant_value(&value, active);
        active.remove(name);
        supported.then_some(value)
    }

    fn is_module_constant_value(
        &self,
        expression: &ast::Expr,
        active: &mut HashSet<String>,
    ) -> bool {
        match expression {
            ast::Expr::Constant(_) => true,
            ast::Expr::UnaryOp(unary) if unary.op == ast::UnaryOp::USub => {
                self.is_module_constant_value(&unary.operand, active)
            }
            ast::Expr::BinOp(binary) => {
                self.is_module_constant_value(&binary.left, active)
                    && self.is_module_constant_value(&binary.right, active)
            }
            ast::Expr::Tuple(tuple) => tuple
                .elts
                .iter()
                .all(|item| self.is_module_constant_value(item, active)),
            ast::Expr::List(list) => list
                .elts
                .iter()
                .all(|item| self.is_module_constant_value(item, active)),
            ast::Expr::Dict(dict) => dict.keys.iter().zip(&dict.values).all(|(key, value)| {
                key.as_ref().is_some_and(|key| {
                    self.is_module_constant_value(key, active)
                        && self.is_module_constant_value(value, active)
                })
            }),
            ast::Expr::Name(name) => self
                .module_constant_expression(name.id.as_str(), active)
                .is_some(),
            _ => false,
        }
    }
}

fn local_assignment_names(statements: &[ast::Stmt]) -> HashSet<String> {
    let mut names = HashSet::new();
    for statement in statements {
        match statement {
            ast::Stmt::Assign(assign) if assign.targets.len() == 1 => {
                collect_target_names(&assign.targets[0], &mut names);
            }
            ast::Stmt::AugAssign(assign) => {
                collect_target_names(&assign.target, &mut names);
            }
            ast::Stmt::While(while_statement) => {
                names.extend(local_assignment_names(&while_statement.body));
            }
            ast::Stmt::If(if_statement) => {
                names.extend(local_assignment_names(&if_statement.body));
                names.extend(local_assignment_names(&if_statement.orelse));
            }
            ast::Stmt::For(for_statement) => {
                collect_target_names(&for_statement.target, &mut names);
                names.extend(local_assignment_names(&for_statement.body));
            }
            _ => {}
        }
    }
    names
}

fn collect_target_names(target: &ast::Expr, names: &mut HashSet<String>) {
    match target {
        ast::Expr::Name(target) => {
            names.insert(target.id.to_string());
        }
        ast::Expr::Tuple(tuple) => {
            for element in &tuple.elts {
                collect_target_names(element, names);
            }
        }
        _ => {}
    }
}

fn ensure_supported_function(
    source: &str,
    function: &ast::StmtFunctionDef,
) -> Result<(), PythonFrontendError> {
    let arguments = &function.args;
    if arguments.vararg.is_some() || !arguments.kwonlyargs.is_empty() || arguments.kwarg.is_some() {
        return Err(error_at(
            source,
            function,
            "variadic and keyword-only parameters are unsupported",
        ));
    }
    if arguments
        .posonlyargs
        .iter()
        .chain(&arguments.args)
        .any(|arg| arg.default.is_some())
    {
        return Err(error_at(
            source,
            function,
            "default parameter values are unsupported",
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
            "decorators, return annotations, and type comments are unsupported",
        ));
    }
    Ok(())
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
