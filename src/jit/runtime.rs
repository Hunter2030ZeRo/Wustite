use std::any::Any;
use std::ffi::c_void;
use std::panic::{AssertUnwindSafe, catch_unwind};

use crate::object::SequenceStrategy;
use crate::value::Value;

mod execute;

pub(super) use execute::{
    EXECUTE_ONE_I64_SYMBOL, EXECUTE_ONE_SYMBOL, EXECUTE_SEQUENCE_ONE_F64_SYMBOL,
    EXECUTE_SEQUENCE_ONE_I64_SYMBOL, EXECUTE_SEQUENCE_ONE_SYMBOL, EXECUTE_SEQUENCE_SYMBOL,
    EXECUTE_SEQUENCE_TWO_SYMBOL, EXECUTE_SYMBOL, EXECUTE_TWO_SYMBOL,
};
pub(super) const SYNC_BOOL_SYMBOL: &str = "wustite_jit_sync_bool";
pub(super) const SYNC_I64_SYMBOL: &str = "wustite_jit_sync_i64";
pub(super) const SYNC_F64_SYMBOL: &str = "wustite_jit_sync_f64";
pub(super) const READ_BOOL_SYMBOL: &str = "wustite_jit_read_bool";
pub(super) const READ_I64_SYMBOL: &str = "wustite_jit_read_i64";
pub(super) const READ_F64_SYMBOL: &str = "wustite_jit_read_f64";
pub(super) const READ_PTR_SYMBOL: &str = "wustite_jit_read_ptr";
pub(super) const SEQUENCE_VIEW_SYMBOL: &str = "wustite_jit_sequence_view";

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub(crate) struct NativeSequenceView {
    pub data: *mut u8,
    pub len: u64,
    pub layout_version: u64,
    pub writable: u64,
}

pub(crate) trait NativeDispatch {
    fn execute(&mut self, registers: &mut [Value], pc: usize) -> Result<(), String>;

    fn execute_sequence(&mut self, registers: &mut [Value], pc: usize) -> Result<(), String> {
        self.execute(registers, pc)
    }

    fn sequence_view(
        &mut self,
        _registers: &[Value],
        _register: u32,
        _expected: SequenceStrategy,
    ) -> Result<NativeSequenceView, String> {
        Err("native sequence views are unavailable".to_string())
    }
}

pub(crate) struct NativeCallContext<'a> {
    registers: &'a mut [Value],
    dispatch: &'a mut dyn NativeDispatch,
    error: Option<String>,
}

impl<'a> NativeCallContext<'a> {
    pub(crate) fn new(registers: &'a mut [Value], dispatch: &'a mut dyn NativeDispatch) -> Self {
        Self {
            registers,
            dispatch,
            error: None,
        }
    }

    pub(crate) fn registers(&mut self) -> &mut [Value] {
        self.registers
    }

    pub(crate) fn take_error(&mut self) -> Option<String> {
        self.error.take()
    }
}

pub(super) const fn symbols() -> [(&'static str, *const u8); 17] {
    [
        (EXECUTE_SYMBOL, execute::execute as *const u8),
        (EXECUTE_ONE_SYMBOL, execute::execute_one as *const u8),
        (EXECUTE_TWO_SYMBOL, execute::execute_two as *const u8),
        (
            EXECUTE_ONE_I64_SYMBOL,
            execute::execute_one_i64 as *const u8,
        ),
        (
            EXECUTE_SEQUENCE_SYMBOL,
            execute::execute_sequence as *const u8,
        ),
        (
            EXECUTE_SEQUENCE_ONE_SYMBOL,
            execute::execute_sequence_one as *const u8,
        ),
        (
            EXECUTE_SEQUENCE_TWO_SYMBOL,
            execute::execute_sequence_two as *const u8,
        ),
        (
            EXECUTE_SEQUENCE_ONE_I64_SYMBOL,
            execute::execute_sequence_one_i64 as *const u8,
        ),
        (
            EXECUTE_SEQUENCE_ONE_F64_SYMBOL,
            execute::execute_sequence_one_f64 as *const u8,
        ),
        (SYNC_BOOL_SYMBOL, sync_bool as *const u8),
        (SYNC_I64_SYMBOL, sync_i64 as *const u8),
        (SYNC_F64_SYMBOL, sync_f64 as *const u8),
        (READ_BOOL_SYMBOL, read_bool as *const u8),
        (READ_I64_SYMBOL, read_i64 as *const u8),
        (READ_F64_SYMBOL, read_f64 as *const u8),
        (READ_PTR_SYMBOL, read_ptr as *const u8),
        (SEQUENCE_VIEW_SYMBOL, sequence_view as *const u8),
    ]
}

extern "C" fn sequence_view(
    context: *mut c_void,
    register: u32,
    expected: u8,
    output: *mut NativeSequenceView,
) -> u8 {
    ffi_boundary(context, |context| {
        let expected = match expected {
            0 => SequenceStrategy::Empty,
            1 => SequenceStrategy::Bool,
            2 => SequenceStrategy::I64,
            3 => SequenceStrategy::F64,
            4 => SequenceStrategy::Object,
            _ => {
                context.error = Some(format!(
                    "native sequence guard has invalid strategy {expected}"
                ));
                return 0;
            }
        };
        let view = match context
            .dispatch
            .sequence_view(context.registers, register, expected)
        {
            Ok(view) => view,
            Err(error) => {
                context.error = Some(error);
                return 0;
            }
        };
        if output.is_null() {
            context.error = Some("native sequence view output pointer is null".to_string());
            return 0;
        }
        // SAFETY: [Categories 3 and 5 - generated-code sequence view output]
        // The backend provides a live aligned NativeSequenceView-sized stack slot.
        unsafe { output.write(view) };
        1
    })
}

extern "C" fn sync_bool(context: *mut c_void, register: u32, value: u8) {
    write_value(context, register, Value::Bool(value != 0));
}

extern "C" fn sync_i64(context: *mut c_void, register: u32, value: i64) {
    write_value(context, register, Value::SmallInt(value));
}

extern "C" fn sync_f64(context: *mut c_void, register: u32, value: f64) {
    write_value(context, register, Value::Float(value));
}

extern "C" fn read_bool(context: *mut c_void, register: u32) -> u8 {
    read_value(context, register, |value| match value {
        Value::Bool(value) => Some(u8::from(value)),
        Value::SmallInt(_)
        | Value::Float(_)
        | Value::None
        | Value::Object(_)
        | Value::Uninitialized => None,
    })
    .unwrap_or_default()
}

extern "C" fn read_i64(context: *mut c_void, register: u32) -> i64 {
    read_value(context, register, |value| match value {
        Value::SmallInt(value) => Some(value),
        Value::Float(_)
        | Value::Bool(_)
        | Value::None
        | Value::Object(_)
        | Value::Uninitialized => None,
    })
    .unwrap_or_default()
}

extern "C" fn read_f64(context: *mut c_void, register: u32) -> f64 {
    read_value(context, register, |value| match value {
        Value::Float(value) => Some(value),
        Value::SmallInt(_)
        | Value::Bool(_)
        | Value::None
        | Value::Object(_)
        | Value::Uninitialized => None,
    })
    .unwrap_or_default()
}

extern "C" fn read_ptr(context: *mut c_void, register: u32) -> u64 {
    ffi_boundary(context, |context| {
        let Some(index) = register_index(context, register) else {
            return 0;
        };
        if context.registers.get(index).is_none() {
            context.error = Some(format!("native JIT read invalid register r{register}"));
            return 0;
        }
        u64::from(register)
    })
}

fn write_value(context: *mut c_void, register: u32, value: Value) {
    ffi_boundary(context, |context| {
        let Some(index) = register_index(context, register) else {
            return;
        };
        let Some(slot) = context.registers.get_mut(index) else {
            context.error = Some(format!("native JIT write invalid register r{register}"));
            return;
        };
        *slot = value;
    });
}

fn read_value<T: Default>(
    context: *mut c_void,
    register: u32,
    convert: impl FnOnce(Value) -> Option<T>,
) -> Option<T> {
    ffi_boundary(context, |context| {
        let index = register_index(context, register)?;
        let value = context.registers.get(index).copied();
        match value.and_then(convert) {
            Some(value) => Some(value),
            None => {
                context.error = Some(format!("native JIT read type mismatch in r{register}"));
                None
            }
        }
    })
}

fn register_index(context: &mut NativeCallContext<'_>, register: u32) -> Option<usize> {
    match usize::try_from(register) {
        Ok(index) => Some(index),
        Err(_) => {
            context.error = Some(format!("native JIT register r{register} exceeds usize"));
            None
        }
    }
}

fn ffi_boundary<T: Default>(
    context: *mut c_void,
    operation: impl FnOnce(&mut NativeCallContext<'_>) -> T,
) -> T {
    if context.is_null() {
        return T::default();
    }
    let outcome = catch_unwind(AssertUnwindSafe(|| {
        // SAFETY: NativeRegionEntry receives the address of a live NativeCallContext,
        // and generated code forwards that address unchanged to every helper.
        operation(unsafe { &mut *context.cast::<NativeCallContext<'_>>() })
    }));
    match outcome {
        Ok(value) => value,
        Err(payload) => {
            // SAFETY: the same live-context invariant used above still holds after
            // catch_unwind intercepts the panic before it crosses the C ABI.
            let context = unsafe { &mut *context.cast::<NativeCallContext<'_>>() };
            context.error = Some(panic_message(payload));
            T::default()
        }
    }
}

fn panic_message(payload: Box<dyn Any + Send>) -> String {
    if let Some(message) = payload.downcast_ref::<&str>() {
        format!("native JIT helper panicked: {message}")
    } else if let Some(message) = payload.downcast_ref::<String>() {
        format!("native JIT helper panicked: {message}")
    } else {
        "native JIT helper panicked".to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct IncrementDispatch;

    impl NativeDispatch for IncrementDispatch {
        fn execute(&mut self, registers: &mut [Value], pc: usize) -> Result<(), String> {
            registers[0] = Value::SmallInt(i64::try_from(pc).map_err(|error| error.to_string())?);
            Ok(())
        }
    }

    #[test]
    fn ffi_helpers_keep_context_reg_provenance() {
        // Given: a live call context backed by a WVM register slice.
        let mut registers = [Value::SmallInt(0), Value::Bool(false)];
        let mut dispatch = IncrementDispatch;
        let mut context = NativeCallContext::new(&mut registers, &mut dispatch);
        let pointer = (&mut context as *mut NativeCallContext<'_>).cast::<c_void>();

        // When: the same helpers used by generated code synchronize and execute values.
        sync_bool(pointer, 1, 1);
        assert_eq!(execute::execute(pointer, 41), 1);

        // Then: typed reads and indirect register pointers observe the same storage.
        assert_eq!(read_i64(pointer, 0), 41);
        assert_eq!(read_bool(pointer, 1), 1);
        assert_eq!(read_ptr(pointer, 0), 0);
        assert!(context.take_error().is_none());
    }

    #[test]
    fn fused_execute_syncs_two_typed_inputs_in_one_boundary() {
        // Given
        let mut registers = [Value::SmallInt(0), Value::Float(0.0)];
        let mut dispatch = IncrementDispatch;
        let mut context = NativeCallContext::new(&mut registers, &mut dispatch);
        let pointer = (&mut context as *mut NativeCallContext<'_>).cast::<c_void>();

        // When
        let status = execute::execute_two(
            pointer,
            7,
            0,
            execute::I64_TAG,
            11,
            1,
            execute::F64_TAG,
            3.5_f64.to_bits(),
        );

        // Then
        assert_eq!(status, 1);
        assert_eq!(registers, [Value::SmallInt(7), Value::Float(3.5)]);
    }

    #[test]
    fn fused_execute_returns_typed_result_via_caller_storage() {
        // Given
        let mut registers = [Value::SmallInt(0), Value::SmallInt(0)];
        let mut dispatch = IncrementDispatch;
        let mut context = NativeCallContext::new(&mut registers, &mut dispatch);
        let pointer = (&mut context as *mut NativeCallContext<'_>).cast::<c_void>();
        // When
        let output = execute::execute_one_i64(pointer, 29, 1, execute::I64_TAG, 4, 0);

        // Then
        assert_eq!(output & 1, 1);
        assert_eq!((output as i64) >> 1, 29);
    }
}
