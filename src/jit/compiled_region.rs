use std::error::Error;
use std::fmt;

use crate::bytecode::Register;
use crate::value::Value;
use crate::wxir::{
    WxExitId, WxExitKind, WxFunction, WxScalarType, WxSideExit, WxStateValue, WxType,
};

use super::layout::RegionLayout;

pub(crate) type NativeRegionEntry = unsafe extern "C" fn(*mut u8) -> u32;

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
        }
    }
}

impl Error for ExecuteError {}

/// Finalized native entry plus its ABI layout and WVM state mappings.
pub struct CompiledRegion {
    entry: NativeRegionEntry,
    layout: RegionLayout,
    entry_state: Vec<WxStateValue>,
    side_exits: Vec<WxSideExit>,
    state_buffer: Vec<u64>,
}

impl CompiledRegion {
    pub(crate) fn new(
        entry: NativeRegionEntry,
        layout: RegionLayout,
        function: &WxFunction,
    ) -> Self {
        let state_buffer = vec![0_u64; layout.word_count().max(1)];
        Self {
            entry,
            layout,
            entry_state: function.entry_state.clone(),
            side_exits: function.side_exits.clone(),
            state_buffer,
        }
    }

    /// Marshals WVM registers, executes native code, and restores exit state.
    pub fn execute(&mut self, registers: &mut [Value]) -> Result<RegionExecution, ExecuteError> {
        self.state_buffer.fill(0);
        for state in &self.entry_state {
            let value = registers
                .get(usize::from(state.register))
                .copied()
                .ok_or(ExecuteError::MissingRegister(state.register))?;
            let word = value_to_word(state.register, state.ty, value)?;
            let index = self
                .layout
                .word_index(state.register)
                .map_err(|error| ExecuteError::Layout(error.to_string()))?;
            self.state_buffer[index] = word;
        }

        // The compiler retains its module; Cranelift's default provider also
        // keeps published code mapped on drop, and `free_memory` is never called.
        // SAFETY: [Categories 3, 5, 6, 8, 10, 14] `entry` has the declared C ABI,
        // RegionLayout bounds buffer accesses, and generated code cannot unwind.
        let raw_exit = unsafe { (self.entry)(self.state_buffer.as_mut_ptr().cast::<u8>()) };
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
            let value = word_to_value(state.ty, self.state_buffer[index]).ok_or(
                ExecuteError::InvalidRegisterType {
                    register: state.register,
                    expected: state.ty,
                    actual: "unsupported native value",
                },
            )?;
            let register = registers
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

fn value_to_word(register: Register, expected: WxType, value: Value) -> Result<u64, ExecuteError> {
    match (expected, value) {
        (WxType::Scalar(WxScalarType::I1), Value::Bool(value)) => Ok(u64::from(value)),
        (WxType::Scalar(WxScalarType::I64), Value::SmallInt(value)) => {
            Ok(u64::from_ne_bytes(value.to_ne_bytes()))
        }
        (_, value) => Err(ExecuteError::EntryTypeMismatch {
            register,
            expected,
            actual: value_name(value),
        }),
    }
}

fn word_to_value(ty: WxType, word: u64) -> Option<Value> {
    match ty {
        WxType::Scalar(WxScalarType::I1) => Some(Value::Bool(word != 0)),
        WxType::Scalar(WxScalarType::I64) => {
            Some(Value::SmallInt(i64::from_ne_bytes(word.to_ne_bytes())))
        }
        _ => None,
    }
}

fn value_name(value: Value) -> &'static str {
    match value {
        Value::SmallInt(_) => "SmallInt",
        Value::Float(_) => "float",
        Value::Object(_) => "object",
        Value::Bool(_) => "bool",
        Value::Uninitialized => "uninitialized",
    }
}
