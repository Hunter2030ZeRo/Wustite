use std::collections::HashSet;

use rustpython_parser::ast;

use super::hir::HirParameter;
use super::{PythonFrontendError, error_at, location_of};
use crate::object::ObjectKind;
use crate::structure_map::SlotType;

pub(super) fn lower_parameters(
    source: &str,
    function: &ast::StmtFunctionDef,
) -> Result<Vec<HirParameter>, PythonFrontendError> {
    let mut names = HashSet::new();
    function
        .args
        .posonlyargs
        .iter()
        .chain(&function.args.args)
        .map(|argument| lower_parameter(source, &argument.def, &mut names))
        .collect()
}

fn lower_parameter(
    source: &str,
    parameter: &ast::Arg,
    names: &mut HashSet<String>,
) -> Result<HirParameter, PythonFrontendError> {
    let name = parameter.arg.to_string();
    if !names.insert(name.clone()) {
        return Err(error_at(
            source,
            parameter,
            format!("parameter `{name}` is defined more than once"),
        ));
    }
    let annotation = parameter.annotation.as_deref().ok_or_else(|| {
        error_at(
            source,
            parameter,
            format!("parameter `{name}` requires a supported annotation"),
        )
    })?;
    let ast::Expr::Name(annotation_name) = annotation else {
        return Err(error_at(
            source,
            annotation,
            format!("parameter `{name}` requires a simple supported annotation"),
        ));
    };
    let ty = annotation_type(annotation_name.id.as_str()).ok_or_else(|| {
        error_at(
            source,
            annotation,
            format!(
                "unsupported annotation `{}` for parameter `{name}`",
                annotation_name.id
            ),
        )
    })?;
    Ok(HirParameter {
        name,
        ty,
        location: location_of(source, parameter),
    })
}

fn annotation_type(name: &str) -> Option<SlotType> {
    match name {
        "int" => Some(SlotType::SmallInt),
        "float" => Some(SlotType::Float),
        "bool" => Some(SlotType::Bool),
        "str" => Some(SlotType::Object(ObjectKind::String)),
        "tuple" => Some(SlotType::Object(ObjectKind::Tuple)),
        "list" => Some(SlotType::Object(ObjectKind::List)),
        "dict" => Some(SlotType::Object(ObjectKind::Dict)),
        "BigInt" | "bigint" => Some(SlotType::Object(ObjectKind::BigInt)),
        "function" => Some(SlotType::Object(ObjectKind::Function)),
        "object" => Some(SlotType::Any),
        _ => None,
    }
}
