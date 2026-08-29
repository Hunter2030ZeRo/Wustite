use std::str::FromStr;

use num_bigint::BigInt;
use wustite::executable::ExecutableParameter;
use wustite::object::{Object, ObjectKind};
use wustite::structure_map::SlotType;
use wustite::{Runtime, RuntimeValue};

pub(super) fn parse_arguments(
    runtime: &mut Runtime,
    parameters: &[ExecutableParameter],
    arguments: &[String],
) -> Result<Vec<RuntimeValue>, String> {
    if arguments.len() != parameters.len() {
        return Err(format!(
            "function expects {} positional argument(s), but {} were provided",
            parameters.len(),
            arguments.len()
        ));
    }

    parameters
        .iter()
        .zip(arguments)
        .enumerate()
        .map(|(index, (parameter, argument))| {
            parse_argument(
                runtime,
                ArgumentInput {
                    index,
                    name: &parameter.name,
                    ty: parameter.ty,
                    value: argument,
                },
            )
        })
        .collect()
}

struct ArgumentInput<'a> {
    index: usize,
    name: &'a str,
    ty: SlotType,
    value: &'a str,
}

fn parse_argument(runtime: &mut Runtime, input: ArgumentInput<'_>) -> Result<RuntimeValue, String> {
    let ArgumentInput {
        index,
        name,
        ty,
        value,
    } = input;
    let invalid = |expected: &str| {
        format!("argument {index} `{name}` with value `{value}` is not a valid {expected}")
    };
    match ty {
        SlotType::SmallInt => value
            .parse::<i64>()
            .map(RuntimeValue::SmallInt)
            .map_err(|_| invalid("small_int")),
        SlotType::Float => value
            .parse::<f64>()
            .map(RuntimeValue::Float)
            .map_err(|_| invalid("float")),
        SlotType::Bool => match value {
            "true" => Ok(RuntimeValue::Bool(true)),
            "false" => Ok(RuntimeValue::Bool(false)),
            _ => Err(invalid("bool (true or false)")),
        },
        SlotType::Object(ObjectKind::String) => allocate(runtime, Object::String(value.to_owned())),
        SlotType::Object(ObjectKind::BigInt) => {
            let big_int = BigInt::from_str(value).map_err(|_| invalid("big_int"))?;
            allocate(runtime, Object::BigInt(big_int))
        }
        SlotType::Object(ObjectKind::Tuple) => Err(unsupported(index, name, "tuple")),
        SlotType::Object(ObjectKind::List) => Err(unsupported(index, name, "list")),
        SlotType::Object(ObjectKind::Dict) => Err(unsupported(index, name, "dict")),
        SlotType::Object(ObjectKind::Function) => Err(unsupported(index, name, "function")),
        SlotType::Object(ObjectKind::Class) => Err(unsupported(index, name, "class")),
        SlotType::Object(ObjectKind::Instance) => Err(unsupported(index, name, "instance")),
        SlotType::Object(ObjectKind::BoundMethod) => Err(unsupported(index, name, "bound_method")),
        SlotType::Any => Err(unsupported(index, name, "Any")),
    }
}

fn allocate(runtime: &mut Runtime, object: Object) -> Result<RuntimeValue, String> {
    runtime
        .allocate_object(object)
        .map(RuntimeValue::Object)
        .map_err(|error| error.to_string())
}

fn unsupported(index: usize, name: &str, ty: &str) -> String {
    format!(
        "argument {index} `{name}` has unsupported CLI type {ty}; construct this value through the Runtime API"
    )
}
