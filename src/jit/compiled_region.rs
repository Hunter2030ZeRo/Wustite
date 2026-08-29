use std::error::Error;
use std::ffi::c_void;
use std::fmt;

use crate::bytecode::Register;
use crate::value::Value;
use crate::wxir::{
    WxExitId, WxExitKind, WxFunction, WxScalarType, WxSideExit, WxStateValue, WxType,
};

use super::layout::RegionLayout;
use super::runtime::{NativeCallContext, NativeDispatch};

pub(crate) type NativeRegionEntry = unsafe extern "C" fn(*mut u8, *mut c_void) -> u32;

pub(crate) enum NativeRegionCode {
    Cranelift(NativeRegionEntry),
    #[cfg(feature = "inkwell")]
    Llvm {
        entry: inkwell::execution_engine::JitFunction<'static, NativeRegionEntry>,
        _context: Box<inkwell::context::Context>,
    },
}

impl NativeRegionCode {
    unsafe fn call(&self, state: *mut u8, context: *mut c_void) -> u32 {
        match self {
            Self::Cranelift(entry) => {
                // SAFETY: [Categories 3, 5, 6, 8, 10, 14] The caller supplies
                // the live RegionLayout-sized state buffer required by this ABI.
                unsafe { entry(state, context) }
            }
            #[cfg(feature = "inkwell")]
            Self::Llvm { entry, .. } => {
                // SAFETY: [Categories 3, 5, 6, 8, 10, 14] JitFunction retains
                // its LLVM execution engine and has the NativeRegionEntry ABI.
                unsafe { entry.call(state, context) }
            }
        }
    }
}

/// Successful native region exit translated back to WVM coordinates.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RegionExecution {
    pub exit: WxExitId,
    pub kind: WxExitKind,
    pub resume_pc: usize,
}

/// A recoverable native-region marshalling or execution failure.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExecuteError {
    MissingRegister(Register),
    EntryTypeMismatch {
        register: Register,
        expected: WxType,
        actual: &'static str,
    },
    InvalidRegisterType {
        register: Register,
        expected: WxType,
        actual: &'static str,
    },
    InvalidExitId(u32),
    Layout(String),
    Runtime(String),
}

impl fmt::Display for ExecuteError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingRegister(register) => {
                write!(formatter, "missing WVM register r{register}")
            }
            Self::EntryTypeMismatch {
                register,
                expected,
                actual,
            }
            | Self::InvalidRegisterType {
                register,
                expected,
                actual,
            } => write!(
                formatter,
                "r{register} contains {actual}, expected {expected}"
            ),
            Self::InvalidExitId(exit) => {
                write!(formatter, "native code returned invalid exit {exit}")
            }
            Self::Layout(error) => formatter.write_str(error),
            Self::Runtime(error) => formatter.write_str(error),
        }
    }
}

impl Error for ExecuteError {}

/// Finalized native entry plus its ABI layout and WVM state mappings.
pub struct CompiledRegion {
    code: NativeRegionCode,
    layout: RegionLayout,
    entry_state: Vec<WxStateValue>,
    side_exits: Vec<WxSideExit>,
    state_buffer: Vec<u64>,
}

impl CompiledRegion {
    pub(crate) fn new(code: NativeRegionCode, layout: RegionLayout, function: &WxFunction) -> Self {
        let state_buffer = vec![0_u64; layout.word_count().max(1)];
        Self {
            code,
            layout,
            entry_state: function.entry_state.clone(),
            side_exits: function.side_exits.clone(),
            state_buffer,
        }
    }

    /// Marshals WVM registers, executes native code, and restores exit state.
    pub fn execute(&mut self, registers: &mut [Value]) -> Result<RegionExecution, ExecuteError> {
        let mut dispatch = UnsupportedNativeDispatch;
        self.execute_with_dispatch(registers, &mut dispatch)
    }

    pub(crate) fn execute_with_dispatch(
        &mut self,
        registers: &mut [Value],
        dispatch: &mut dyn NativeDispatch,
    ) -> Result<RegionExecution, ExecuteError> {
        let mut context = NativeCallContext::new(registers, dispatch);
        self.state_buffer.fill(0);
        for state in &self.entry_state {
            let value = context
                .registers()
                .get_mut(usize::from(state.register))
                .ok_or(ExecuteError::MissingRegister(state.register))?;
            let word = encode_state_value(state.register, state.ty, value)?;
            let index = self
                .layout
                .word_index(state.register)
                .map_err(|error| ExecuteError::Layout(error.to_string()))?;
            self.state_buffer[index] = word;
        }

        // SAFETY: [Categories 3, 5, 6, 8, 10, 14] `entry` has the declared C ABI,
        // NativeRegionCode retains backend code ownership, RegionLayout bounds
        // buffer accesses, and generated code cannot unwind.
        let context_pointer = (&mut context as *mut NativeCallContext<'_>).cast::<c_void>();
        let raw_exit = unsafe {
            self.code
                .call(self.state_buffer.as_mut_ptr().cast::<u8>(), context_pointer)
        };
        if let Some(error) = context.take_error() {
            return Err(ExecuteError::Runtime(error));
        }
        let exit = WxExitId(raw_exit);
        let metadata = self
            .side_exits
            .iter()
            .find(|metadata| metadata.id == exit)
            .ok_or(ExecuteError::InvalidExitId(raw_exit))?;

        for state in &metadata.state {
            let index = self
                .layout
                .word_index(state.register)
                .map_err(|error| ExecuteError::Layout(error.to_string()))?;
            let value = decode_state_value(state.ty, self.state_buffer[index], context.registers())
                .ok_or(ExecuteError::InvalidRegisterType {
                    register: state.register,
                    expected: state.ty,
                    actual: "unsupported native value",
                })?;
            let register = context
                .registers()
                .get_mut(usize::from(state.register))
                .ok_or(ExecuteError::MissingRegister(state.register))?;
            *register = value;
        }

        Ok(RegionExecution {
            exit,
            kind: metadata.kind,
            resume_pc: metadata.resume_pc,
        })
    }

    /// Native state layout retained by this compiled region.
    pub fn layout(&self) -> &RegionLayout {
        &self.layout
    }
}

fn encode_state_value(
    register: Register,
    expected: WxType,
    value: &mut Value,
) -> Result<u64, ExecuteError> {
    match (expected, *value) {
        (WxType::Scalar(WxScalarType::I1), Value::Bool(value)) => Ok(u64::from(value)),
        (WxType::Scalar(WxScalarType::I64), Value::SmallInt(value)) => {
            Ok(u64::from_ne_bytes(value.to_ne_bytes()))
        }
        (WxType::Scalar(WxScalarType::F64), Value::Float(value)) => Ok(value.to_bits()),
        (WxType::Scalar(WxScalarType::RuntimeHandle), _) => Ok(u64::from(register)),
        (_, value) => Err(ExecuteError::EntryTypeMismatch {
            register,
            expected,
            actual: value_name(value),
        }),
    }
}

fn decode_state_value(ty: WxType, word: u64, registers: &[Value]) -> Option<Value> {
    match ty {
        WxType::Scalar(WxScalarType::I1) => Some(Value::Bool(word != 0)),
        WxType::Scalar(WxScalarType::I64) => {
            Some(Value::SmallInt(i64::from_ne_bytes(word.to_ne_bytes())))
        }
        WxType::Scalar(WxScalarType::F64) => Some(Value::Float(f64::from_bits(word))),
        WxType::Scalar(WxScalarType::RuntimeHandle) => {
            registers.get(usize::try_from(word).ok()?).copied()
        }
        _ => None,
    }
}

struct UnsupportedNativeDispatch;

impl NativeDispatch for UnsupportedNativeDispatch {
    fn execute(&mut self, _registers: &mut [Value], _pc: usize) -> Result<(), String> {
        Err("compiled region requires a WVM native dispatch context".to_string())
    }
}

fn value_name(value: Value) -> &'static str {
    match value {
        Value::SmallInt(_) => "SmallInt",
        Value::Float(_) => "float",
        Value::Object(_) => "object",
        Value::Bool(_) => "bool",
        Value::None => "none",
        Value::Uninitialized => "uninitialized",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn handle_state_round_trips_a_live_register() {
        // Given: a live WVM value encoded through the runtime-handle ABI.
        let mut value = Value::SmallInt(42);
        let ty = WxType::Scalar(WxScalarType::RuntimeHandle);

        // When: native state encoding records and resolves its register handle.
        let word = encode_state_value(0, ty, &mut value).unwrap();
        let decoded = decode_state_value(ty, word, std::slice::from_ref(&value));

        // Then: the exact register value survives the handle round trip.
        assert_eq!(decoded, Some(Value::SmallInt(42)));
    }
}
