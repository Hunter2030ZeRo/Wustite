use crate::bytecode::Register;
use crate::value::Value;

use super::{Frame, Vm};

pub(super) fn read_small_int(frame: &Frame, register: Register) -> Result<i64, String> {
    match Vm::read_register(frame, register)? {
        Value::SmallInt(value) => Ok(value),
        other => Err(format!("expected SmallInt in r{register}, found {other:?}")),
    }
}

pub(super) fn read_bool(frame: &Frame, register: Register) -> Result<bool, String> {
    match Vm::read_register(frame, register)? {
        Value::Bool(value) => Ok(value),
        other => Err(format!("expected bool in r{register}, found {other:?}")),
    }
}
