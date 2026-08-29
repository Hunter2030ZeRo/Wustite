use crate::executable::{ExecutableFunction, ExecutableParameter};
use crate::object::{ObjectHeap, ObjectKind};
use crate::structure_map::SlotType;
use crate::value::Value;

pub(crate) fn initialize_registers(
    executable: &ExecutableFunction,
    arguments: &[Value],
    heap: &ObjectHeap,
    registers: &mut [Value],
) -> Result<(), String> {
    let parameters = executable.parameters();
    if arguments.len() != parameters.len() {
        return Err(format!(
            "function expected {} arguments, got {}",
            parameters.len(),
            arguments.len()
        ));
    }

    if registers.len() != executable.bytecode().register_count {
        return Err("function frame has the wrong register count".to_string());
    }
    registers.fill(Value::Uninitialized);
    for (index, (parameter, argument)) in parameters.iter().zip(arguments).enumerate() {
        validate_argument(index, parameter, *argument, heap)?;
        registers[usize::from(parameter.register)] = *argument;
    }
    Ok(())
}

fn validate_argument(
    index: usize,
    parameter: &ExecutableParameter,
    argument: Value,
    heap: &ObjectHeap,
) -> Result<(), String> {
    let valid = match (parameter.ty, argument) {
        (SlotType::SmallInt, Value::SmallInt(_))
        | (SlotType::Float, Value::Float(_))
        | (SlotType::Bool, Value::Bool(_)) => true,
        (SlotType::Any, Value::SmallInt(_) | Value::Float(_) | Value::Bool(_) | Value::None) => {
            true
        }
        (SlotType::Any, Value::Object(reference)) => {
            heap.kind(reference).map_err(|error| {
                format!(
                    "argument {index} `{}`: expected {}, got invalid object reference: {error}",
                    parameter.name,
                    type_name(parameter.ty)
                )
            })?;
            true
        }
        (SlotType::Object(expected), Value::Object(reference)) => {
            let actual = heap.kind(reference).map_err(|error| {
                format!(
                    "argument {index} `{}`: expected {}, got invalid object reference: {error}",
                    parameter.name,
                    type_name(parameter.ty)
                )
            })?;
            expected == actual
        }
        (
            SlotType::SmallInt,
            Value::Float(_)
            | Value::Bool(_)
            | Value::None
            | Value::Object(_)
            | Value::Uninitialized,
        )
        | (
            SlotType::Float,
            Value::SmallInt(_)
            | Value::Bool(_)
            | Value::None
            | Value::Object(_)
            | Value::Uninitialized,
        )
        | (
            SlotType::Bool,
            Value::SmallInt(_)
            | Value::Float(_)
            | Value::None
            | Value::Object(_)
            | Value::Uninitialized,
        )
        | (
            SlotType::Object(_),
            Value::SmallInt(_)
            | Value::Float(_)
            | Value::Bool(_)
            | Value::None
            | Value::Uninitialized,
        )
        | (SlotType::Any, Value::Uninitialized) => false,
    };

    if valid {
        Ok(())
    } else {
        Err(format!(
            "argument {index} `{}`: expected {}, got {}",
            parameter.name,
            type_name(parameter.ty),
            value_name(argument, heap)
        ))
    }
}

const fn type_name(ty: SlotType) -> &'static str {
    match ty {
        SlotType::SmallInt => "small_int",
        SlotType::Float => "float",
        SlotType::Bool => "bool",
        SlotType::Object(kind) => object_kind_name(kind),
        SlotType::Any => "any initialized value",
    }
}

const fn object_kind_name(kind: ObjectKind) -> &'static str {
    match kind {
        ObjectKind::String => "string",
        ObjectKind::Tuple => "tuple",
        ObjectKind::BigInt => "big_int",
        ObjectKind::List => "list",
        ObjectKind::Dict => "dict",
        ObjectKind::Function => "function",
        ObjectKind::Class => "class",
        ObjectKind::Instance => "instance",
        ObjectKind::BoundMethod => "bound_method",
    }
}

fn value_name(value: Value, heap: &ObjectHeap) -> &'static str {
    match value {
        Value::SmallInt(_) => "small_int",
        Value::Float(_) => "float",
        Value::Bool(_) => "bool",
        Value::None => "none",
        Value::Object(reference) => match heap.kind(reference) {
            Ok(kind) => object_kind_name(kind),
            Err(_) => "invalid object reference",
        },
        Value::Uninitialized => "uninitialized",
    }
}
