use crate::bytecode::{Instruction, Register};
use crate::object::{Object, ObjectHeap};
use crate::value::Value;

mod slice;

pub(crate) use slice::{fuse_reverse_prefix, match_reverse_prefix, temporary_is_dead};

pub(super) fn execute_small_int_sequence_access(
    heap: &mut ObjectHeap,
    registers: &mut [Value],
    instruction: &Instruction,
) -> Result<bool, String> {
    match instruction {
        Instruction::GetItem { dst, object, key } => get_item(heap, registers, *dst, *object, *key),
        Instruction::SetItem {
            object, key, value, ..
        } => set_item(heap, registers, *object, *key, *value),
        Instruction::Length { dst, object } => length(heap, registers, *dst, *object),
        Instruction::ListAppend { list, value } => append(heap, registers, *list, *value),
        Instruction::ListInsert { list, index, value } => {
            insert(heap, registers, *list, *index, *value)
        }
        Instruction::ListPop { dst, list, index } => pop(heap, registers, *dst, *list, *index),
        Instruction::GetSlice {
            dst,
            object,
            start,
            stop,
            step,
        } => slice::get(heap, registers, *dst, *object, *start, *stop, *step),
        Instruction::SetSlice {
            object,
            start,
            stop,
            step,
            value,
        } => slice::set(heap, registers, *object, *start, *stop, *step, *value),
        _ => Ok(false),
    }
}

fn length(
    heap: &ObjectHeap,
    registers: &mut [Value],
    dst: Register,
    object: Register,
) -> Result<bool, String> {
    let Some(Value::Object(reference)) = read(registers, object) else {
        return Ok(false);
    };
    let length = match heap.get(reference).map_err(|error| error.to_string())? {
        Object::List(values) | Object::Tuple(values) => values.len(),
        _ => return Ok(false),
    };
    let length = i64::try_from(length).map_err(|_| "object length exceeds SmallInt range")?;
    write(registers, dst, Value::SmallInt(length))?;
    Ok(true)
}

fn append(
    heap: &mut ObjectHeap,
    registers: &[Value],
    list: Register,
    value: Register,
) -> Result<bool, String> {
    let (Some(Value::Object(reference)), Some(value)) =
        (read(registers, list), read(registers, value))
    else {
        return Ok(false);
    };
    let Object::List(values) = heap.get_mut(reference).map_err(|error| error.to_string())? else {
        return Ok(false);
    };
    values.push(value);
    Ok(true)
}

fn get_item(
    heap: &ObjectHeap,
    registers: &mut [Value],
    dst: Register,
    object: Register,
    key: Register,
) -> Result<bool, String> {
    let (Some(Value::Object(reference)), Some(Value::SmallInt(index))) =
        (read(registers, object), read(registers, key))
    else {
        return Ok(false);
    };
    let value = match heap.get(reference).map_err(|error| error.to_string())? {
        Object::List(values) | Object::Tuple(values) => values
            .get(sequence_index(index, values.len())?)
            .ok_or_else(|| "sequence index out of range".to_string())?,
        _ => return Ok(false),
    };
    write(registers, dst, value)?;
    Ok(true)
}

fn set_item(
    heap: &mut ObjectHeap,
    registers: &[Value],
    object: Register,
    key: Register,
    value: Register,
) -> Result<bool, String> {
    let (Some(Value::Object(reference)), Some(Value::SmallInt(index)), Some(value)) = (
        read(registers, object),
        read(registers, key),
        read(registers, value),
    ) else {
        return Ok(false);
    };
    let Object::List(values) = heap.get_mut(reference).map_err(|error| error.to_string())? else {
        return Ok(false);
    };
    let index = sequence_index(index, values.len())?;
    let _ = values.set(index, value);
    Ok(true)
}

fn insert(
    heap: &mut ObjectHeap,
    registers: &[Value],
    list: Register,
    index: Register,
    value: Register,
) -> Result<bool, String> {
    let (Some(Value::Object(reference)), Some(Value::SmallInt(index)), Some(value)) = (
        read(registers, list),
        read(registers, index),
        read(registers, value),
    ) else {
        return Ok(false);
    };
    let Object::List(values) = heap.get_mut(reference).map_err(|error| error.to_string())? else {
        return Ok(false);
    };
    let length = i64::try_from(values.len()).map_err(|_| "list is too large".to_string())?;
    let index = if index < 0 {
        length.saturating_add(index).max(0)
    } else {
        index.min(length)
    };
    values.insert(
        usize::try_from(index).map_err(|_| "invalid list index".to_string())?,
        value,
    );
    Ok(true)
}

fn pop(
    heap: &mut ObjectHeap,
    registers: &mut [Value],
    dst: Register,
    list: Register,
    index: Register,
) -> Result<bool, String> {
    let (Some(Value::Object(reference)), Some(Value::SmallInt(index))) =
        (read(registers, list), read(registers, index))
    else {
        return Ok(false);
    };
    let Object::List(values) = heap.get_mut(reference).map_err(|error| error.to_string())? else {
        return Ok(false);
    };
    let value = values
        .remove(sequence_index(index, values.len())?)
        .ok_or_else(|| "sequence index out of range".to_string())?;
    write(registers, dst, value)?;
    Ok(true)
}

fn read(registers: &[Value], register: Register) -> Option<Value> {
    registers.get(usize::from(register)).copied()
}

fn write(registers: &mut [Value], register: Register, value: Value) -> Result<(), String> {
    *registers
        .get_mut(usize::from(register))
        .ok_or_else(|| format!("missing register r{register}"))? = value;
    Ok(())
}

fn sequence_index(index: i64, length: usize) -> Result<usize, String> {
    let length = i64::try_from(length).map_err(|_| "sequence length exceeds i64".to_string())?;
    let index = (if index < 0 {
        length.checked_add(index)
    } else {
        Some(index)
    })
    .filter(|index| *index >= 0 && *index < length)
    .ok_or_else(|| "sequence index out of range".to_string())?;
    usize::try_from(index).map_err(|error| error.to_string())
}
