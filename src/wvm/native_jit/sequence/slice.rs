use crate::bytecode::{Instruction, Register};
use crate::object::{Object, ObjectHeap};
use crate::value::Value;

use super::{read, write};

mod liveness;

pub(crate) use liveness::temporary_is_dead;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ReversePrefixPattern {
    pub(crate) object: Register,
    pub(crate) start: Register,
    pub(crate) step: Register,
    pub(crate) stop: Register,
    pub(crate) temporary: Register,
}

pub(crate) fn match_reverse_prefix(
    code: &[Instruction],
    pc: usize,
) -> Option<ReversePrefixPattern> {
    let Instruction::GetSlice {
        dst,
        object,
        start: Some(start),
        stop: None,
        step: Some(step),
    } = code.get(pc)?
    else {
        return None;
    };
    let Instruction::SetSlice {
        object: target,
        start: None,
        stop: Some(stop),
        step: None,
        value,
    } = code.get(pc.checked_add(1)?)?
    else {
        return None;
    };
    if object != target
        || dst != value
        || !liveness::temporary_is_dead(code, pc.saturating_add(2), *dst)
    {
        return None;
    }
    Some(ReversePrefixPattern {
        object: *object,
        start: *start,
        step: *step,
        stop: *stop,
        temporary: *dst,
    })
}

pub(crate) fn fuse_reverse_prefix(
    code: &[Instruction],
    pc: usize,
    heap: &mut ObjectHeap,
    registers: &mut [Value],
) -> Result<bool, String> {
    let Some(pattern) = match_reverse_prefix(code, pc) else {
        return Ok(false);
    };
    let (
        Some(Value::Object(reference)),
        Some(Value::SmallInt(start)),
        Some(Value::SmallInt(-1)),
        Some(Value::SmallInt(stop)),
    ) = (
        read(registers, pattern.object),
        read(registers, pattern.start),
        read(registers, pattern.step),
        read(registers, pattern.stop),
    )
    else {
        return Ok(false);
    };
    if start < 0 || stop != start.saturating_add(1) {
        return Ok(false);
    }
    let Object::List(values) = heap.get_mut(reference).map_err(|error| error.to_string())? else {
        return Ok(false);
    };
    let end = usize::try_from(stop).map_err(|error| error.to_string())?;
    if !values.reverse_prefix(end) {
        return Ok(false);
    }
    write(registers, pattern.temporary, Value::Object(reference))?;
    Ok(true)
}

pub(super) fn get(
    heap: &mut ObjectHeap,
    registers: &mut [Value],
    dst: Register,
    object: Register,
    start: Option<Register>,
    stop: Option<Register>,
    step: Option<Register>,
) -> Result<bool, String> {
    let Some(Value::Object(reference)) = read(registers, object) else {
        return Ok(false);
    };
    let (Some(start), Some(stop), Some(step)) = (
        optional_small_int(registers, start)?,
        optional_small_int(registers, stop)?,
        optional_small_int(registers, step)?,
    ) else {
        return Ok(false);
    };
    let sliced = match heap.get(reference).map_err(|error| error.to_string())? {
        Object::List(values) => Object::list(
            super::super::super::objects::slice_indices(heap, values.len(), start, stop, step)?
                .into_iter()
                .map(|index| values.get(index).expect("validated slice index"))
                .collect(),
        ),
        Object::Tuple(values) => Object::tuple(
            super::super::super::objects::slice_indices(heap, values.len(), start, stop, step)?
                .into_iter()
                .map(|index| values.get(index).expect("validated slice index"))
                .collect(),
        ),
        _ => return Ok(false),
    };
    write(
        registers,
        dst,
        Value::Object(heap.allocate(sliced).map_err(|error| error.to_string())?),
    )?;
    Ok(true)
}

#[allow(clippy::too_many_arguments)]
pub(super) fn set(
    heap: &mut ObjectHeap,
    registers: &[Value],
    object: Register,
    start: Option<Register>,
    stop: Option<Register>,
    step: Option<Register>,
    value: Register,
) -> Result<bool, String> {
    let (Some(Value::Object(reference)), Some(Value::Object(replacement))) =
        (read(registers, object), read(registers, value))
    else {
        return Ok(false);
    };
    let (Some(start), Some(stop), Some(step)) = (
        optional_small_int(registers, start)?,
        optional_small_int(registers, stop)?,
        optional_small_int(registers, step)?,
    ) else {
        return Ok(false);
    };
    if step.is_some_and(|value| value != Value::SmallInt(1)) {
        return Ok(false);
    }
    let replacement = match heap.get(replacement).map_err(|error| error.to_string())? {
        Object::List(values) | Object::Tuple(values) => values.to_vec(),
        _ => return Ok(false),
    };
    let length = match heap.get(reference).map_err(|error| error.to_string())? {
        Object::List(values) => values.len(),
        _ => return Ok(false),
    };
    let (start, stop) =
        super::super::super::objects::forward_slice_bounds(heap, length, start, stop)?;
    let Object::List(values) = heap.get_mut(reference).map_err(|error| error.to_string())? else {
        return Ok(false);
    };
    values.replace_range(start..stop, replacement);
    Ok(true)
}

fn optional_small_int(
    registers: &[Value],
    register: Option<Register>,
) -> Result<Option<Option<Value>>, String> {
    let Some(register) = register else {
        return Ok(Some(None));
    };
    match read(registers, register) {
        Some(value @ Value::SmallInt(_)) => Ok(Some(Some(value))),
        Some(_) => Ok(None),
        None => Err(format!("missing register r{register}")),
    }
}

#[cfg(test)]
mod tests {
    use super::match_reverse_prefix;
    use crate::bytecode::Instruction;

    fn pair() -> Vec<Instruction> {
        vec![
            Instruction::GetSlice {
                dst: 4,
                object: 0,
                start: Some(1),
                stop: None,
                step: Some(2),
            },
            Instruction::SetSlice {
                object: 0,
                start: None,
                stop: Some(3),
                step: None,
                value: 4,
            },
        ]
    }

    #[test]
    fn reverse_prefix_matcher_preserves_exact_pair_and_dead_temporary() {
        let code = pair();
        let pattern = match_reverse_prefix(&code, 0).expect("exact adjacent pair");
        assert_eq!(pattern.object, 0);
        assert_eq!(pattern.start, 1);
        assert_eq!(pattern.step, 2);
        assert_eq!(pattern.stop, 3);
        assert_eq!(pattern.temporary, 4);
    }

    #[test]
    fn reverse_prefix_matcher_rejects_alias_and_liveness_near_misses() {
        let mut wrong_target = pair();
        let Instruction::SetSlice { object, .. } = &mut wrong_target[1] else {
            unreachable!()
        };
        *object = 5;
        assert!(match_reverse_prefix(&wrong_target, 0).is_none());

        let mut wrong_value = pair();
        let Instruction::SetSlice { value, .. } = &mut wrong_value[1] else {
            unreachable!()
        };
        *value = 5;
        assert!(match_reverse_prefix(&wrong_value, 0).is_none());

        let mut live_temporary = pair();
        live_temporary.push(Instruction::Move { dst: 5, src: 4 });
        assert!(match_reverse_prefix(&live_temporary, 0).is_none());
    }
}
