//! A deliberately small Python-to-WVM frontend.

mod error;
mod expression;
mod hir;
mod lower;
mod parameters;
mod statements;

use std::collections::{HashMap, HashSet};

pub use error::{PythonFrontendError, SourceLocation};

use rustpython_parser::Parse;
use rustpython_parser::ast::{self, Ranged};

use crate::executable::ExecutableFunction;

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
    }
    .compile_named(function_name, None)
}

struct Compiler<'a> {
    source: &'a str,
    suite: &'a [ast::Stmt],
    stack: Vec<String>,
    compiled: HashMap<String, ExecutableFunction>,
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
            false,
            current_name,
            &local_names,
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
        lower::lower(HirFunction { parameters, body })
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
}

fn local_assignment_names(statements: &[ast::Stmt]) -> HashSet<String> {
    let mut names = HashSet::new();
    for statement in statements {
        match statement {
            ast::Stmt::Assign(assign) if assign.targets.len() == 1 => {
                if let ast::Expr::Name(target) = &assign.targets[0] {
                    names.insert(target.id.to_string());
                }
            }
            ast::Stmt::While(while_statement) => {
                names.extend(local_assignment_names(&while_statement.body));
            }
            _ => {}
        }
    }
    names
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
