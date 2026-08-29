use std::ffi::c_void;

use crate::value::Value;

use super::{NativeCallContext, ffi_boundary, register_index};

pub(crate) const EXECUTE_SYMBOL: &str = "wustite_jit_execute";
pub(crate) const EXECUTE_ONE_SYMBOL: &str = "wustite_jit_execute_one";
pub(crate) const EXECUTE_TWO_SYMBOL: &str = "wustite_jit_execute_two";
pub(crate) const EXECUTE_ONE_I64_SYMBOL: &str = "wustite_jit_execute_one_i64";
pub(crate) const EXECUTE_SEQUENCE_SYMBOL: &str = "wustite_jit_execute_sequence";
pub(crate) const EXECUTE_SEQUENCE_ONE_SYMBOL: &str = "wustite_jit_execute_sequence_one";
pub(crate) const EXECUTE_SEQUENCE_TWO_SYMBOL: &str = "wustite_jit_execute_sequence_two";
pub(crate) const EXECUTE_SEQUENCE_ONE_I64_SYMBOL: &str = "wustite_jit_execute_sequence_one_i64";
pub(crate) const EXECUTE_SEQUENCE_ONE_F64_SYMBOL: &str = "wustite_jit_execute_sequence_one_f64";

pub(super) const BOOL_TAG: u8 = 0;
pub(super) const I64_TAG: u8 = 1;
pub(super) const F64_TAG: u8 = 2;

pub(super) extern "C" fn execute(context: *mut c_void, pc: u32) -> u8 {
    execute_with(context, pc, &[], false, |_| true)
}

pub(super) extern "C" fn execute_one(
    context: *mut c_void,
    pc: u32,
    register: u32,
    tag: u8,
    bits: u64,
) -> u8 {
    execute_with(context, pc, &[(register, tag, bits)], false, |_| true)
}

#[allow(clippy::too_many_arguments)]
pub(super) extern "C" fn execute_two(
    context: *mut c_void,
    pc: u32,
    register0: u32,
    tag0: u8,
    bits0: u64,
    register1: u32,
    tag1: u8,
    bits1: u64,
) -> u8 {
    execute_with(
        context,
        pc,
        &[(register0, tag0, bits0), (register1, tag1, bits1)],
        false,
        |_| true,
    )
}

#[allow(clippy::too_many_arguments)]
pub(super) extern "C" fn execute_one_i64(
    context: *mut c_void,
    pc: u32,
    input_register: u32,
    input_tag: u8,
    input_bits: u64,
    output_register: u32,
) -> u64 {
    ffi_boundary(context, |context| {
        if !run(
            context,
            pc,
            &[(input_register, input_tag, input_bits)],
            false,
        ) {
            return 0;
        }
        let Some(index) = register_index(context, output_register) else {
            return 0;
        };
        let Some(Value::SmallInt(value)) = context.registers.get(index).copied() else {
            context.error = Some(format!(
                "native JIT read type mismatch in r{output_register}"
            ));
            return 0;
        };
        if !(i64::MIN / 2..=i64::MAX / 2).contains(&value) {
            context.error = Some("native JIT packed integer result exceeds 63 bits".to_string());
            return 0;
        }
        (value as u64).wrapping_shl(1) | 1
    })
}

pub(super) extern "C" fn execute_sequence(context: *mut c_void, pc: u32) -> u8 {
    execute_with(context, pc, &[], true, |_| true)
}

pub(super) extern "C" fn execute_sequence_one(
    context: *mut c_void,
    pc: u32,
    register: u32,
    tag: u8,
    bits: u64,
) -> u8 {
    execute_with(context, pc, &[(register, tag, bits)], true, |_| true)
}

#[allow(clippy::too_many_arguments)]
pub(super) extern "C" fn execute_sequence_two(
    context: *mut c_void,
    pc: u32,
    register0: u32,
    tag0: u8,
    bits0: u64,
    register1: u32,
    tag1: u8,
    bits1: u64,
) -> u8 {
    execute_with(
        context,
        pc,
        &[(register0, tag0, bits0), (register1, tag1, bits1)],
        true,
        |_| true,
    )
}

#[allow(clippy::too_many_arguments)]
pub(super) extern "C" fn execute_sequence_one_i64(
    context: *mut c_void,
    pc: u32,
    input_register: u32,
    input_tag: u8,
    input_bits: u64,
    output_register: u32,
) -> u64 {
    ffi_boundary(context, |context| {
        if !run(
            context,
            pc,
            &[(input_register, input_tag, input_bits)],
            true,
        ) {
            return 0;
        }
        pack_i64_result(context, output_register)
    })
}

#[allow(clippy::too_many_arguments)]
pub(super) extern "C" fn execute_sequence_one_f64(
    context: *mut c_void,
    pc: u32,
    input_register: u32,
    input_tag: u8,
    input_bits: u64,
    output_register: u32,
    output: *mut f64,
) -> u8 {
    execute_with(
        context,
        pc,
        &[(input_register, input_tag, input_bits)],
        true,
        |context| {
            let Some(index) = register_index(context, output_register) else {
                return false;
            };
            let Some(Value::Float(value)) = context.registers.get(index).copied() else {
                context.error = Some(format!(
                    "native JIT read type mismatch in r{output_register}"
                ));
                return false;
            };
            if output.is_null() {
                context.error = Some("native JIT float output pointer is null".to_string());
                return false;
            }
            // SAFETY: [Categories 3 and 5 - generated-code scratch output]
            // Both backends pass an aligned live f64 stack slot for this call only.
            unsafe { output.write(value) };
            true
        },
    )
}

fn execute_with(
    context: *mut c_void,
    pc: u32,
    inputs: &[(u32, u8, u64)],
    sequence: bool,
    after: impl FnOnce(&mut NativeCallContext<'_>) -> bool,
) -> u8 {
    ffi_boundary(context, |context| {
        u8::from(run(context, pc, inputs, sequence) && after(context))
    })
}

fn run(
    context: &mut NativeCallContext<'_>,
    pc: u32,
    inputs: &[(u32, u8, u64)],
    sequence: bool,
) -> bool {
    if context.error.is_some() {
        return false;
    }
    for &(register, tag, bits) in inputs {
        let Some(index) = register_index(context, register) else {
            return false;
        };
        let Some(slot) = context.registers.get_mut(index) else {
            context.error = Some(format!("native JIT write invalid register r{register}"));
            return false;
        };
        *slot = match tag {
            BOOL_TAG => Value::Bool(bits != 0),
            I64_TAG => Value::SmallInt(bits as i64),
            F64_TAG => Value::Float(f64::from_bits(bits)),
            _ => {
                context.error = Some(format!("native JIT input has invalid type tag {tag}"));
                return false;
            }
        };
    }
    let Ok(pc) = usize::try_from(pc) else {
        context.error = Some("native JIT bytecode pc exceeds usize".to_string());
        return false;
    };
    let result = if sequence {
        context.dispatch.execute_sequence(context.registers, pc)
    } else {
        context.dispatch.execute(context.registers, pc)
    };
    if let Err(error) = result {
        context.error = Some(format!("runtime instruction pc {pc}: {error}"));
        return false;
    }
    true
}

fn pack_i64_result(context: &mut NativeCallContext<'_>, output_register: u32) -> u64 {
    let Some(index) = register_index(context, output_register) else {
        return 0;
    };
    let Some(Value::SmallInt(value)) = context.registers.get(index).copied() else {
        context.error = Some(format!(
            "native JIT read type mismatch in r{output_register}"
        ));
        return 0;
    };
    if !(i64::MIN / 2..=i64::MAX / 2).contains(&value) {
        context.error = Some("native JIT packed integer result exceeds 63 bits".to_string());
        return 0;
    }
    (value as u64).wrapping_shl(1) | 1
}
