mod control;
mod lowering;
mod operations;
mod setup;
mod state;

use std::collections::{HashMap, HashSet, VecDeque};
use std::error::Error;
use std::fmt;

use crate::bytecode::{BinaryOperator, CompareOperator, Instruction, Register};
use crate::executable::ExecutableFunction;
use crate::planner::JitPlan;
use crate::structure_map::{LiveSlot, OperationSiteId, SlotType, TypeFact};
use crate::verifier as wvm_verifier;

use super::ir::{
    WxBlock, WxBlockId, WxBlockParam, WxBlockTarget, WxCompareOp, WxExitId, WxExitKind, WxFunction,
    WxGuardMode, WxInst, WxInstKind, WxInstResult, WxIntCompareOp, WxIntOverflowOp, WxRegionOrigin,
    WxSideExit, WxStateValue, WxTerminator, WxValueId,
};
use super::types::{WxScalarType, WxType};
use super::verifier as wxir_verifier;

/// A recoverable failure while translating WVM bytecode into WXIR.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WxBuildError {
    InvalidExecutable(String),
    InvalidPlan(String),
    UnsupportedInstruction {
        pc: usize,
        instruction: &'static str,
    },
    UnsupportedSpecialization {
        pc: usize,
        reason: String,
    },
    MissingRegister {
        pc: usize,
        register: Register,
    },
    TypeMismatch {
        pc: usize,
        register: Register,
        expected: WxType,
        actual: WxType,
    },
    IdSpaceExhausted(&'static str),
    InvalidWxir(String),
}

impl fmt::Display for WxBuildError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidExecutable(error) => write!(formatter, "invalid executable: {error}"),
            Self::InvalidPlan(error) => write!(formatter, "invalid JIT plan: {error}"),
            Self::UnsupportedInstruction { pc, instruction } => {
                write!(
                    formatter,
                    "unsupported WVM instruction {instruction} at pc {pc}"
                )
            }
            Self::UnsupportedSpecialization { pc, reason } => {
                write!(
                    formatter,
                    "cannot specialize WVM operation at pc {pc}: {reason}"
                )
            }
            Self::MissingRegister { pc, register } => {
                write!(formatter, "r{register} has no WXIR value at pc {pc}")
            }
            Self::TypeMismatch {
                pc,
                register,
                expected,
                actual,
            } => write!(
                formatter,
                "r{register} has type {actual} at pc {pc}, expected {expected}"
            ),
            Self::IdSpaceExhausted(kind) => write!(formatter, "{kind} ID space exhausted"),
            Self::InvalidWxir(error) => write!(formatter, "generated invalid WXIR: {error}"),
        }
    }
}

impl Error for WxBuildError {}

/// Builds and verifies typed SSA for one planned WVM hot region.
pub fn build_region(
    executable: &ExecutableFunction,
    plan: &JitPlan,
) -> Result<WxFunction, WxBuildError> {
    wvm_verifier::verify(executable).map_err(WxBuildError::InvalidExecutable)?;
    verify_plan(executable, plan)?;

    let mut builder = RegionBuilder::new(executable, plan)?;
    let function = builder.build()?;
    wxir_verifier::verify(&function).map_err(WxBuildError::InvalidWxir)?;
    Ok(function)
}

fn verify_plan(executable: &ExecutableFunction, plan: &JitPlan) -> Result<(), WxBuildError> {
    let region = executable
        .structure_map()
        .loops
        .get(plan.region_id.0)
        .ok_or_else(|| {
            WxBuildError::InvalidPlan(format!("unknown region ID {}", plan.region_id.0))
        })?;

    if region.header != plan.header
        || region.backedge != plan.backedge
        || region.exits != plan.exits
        || region.live_slots != plan.live_slots
    {
        return Err(WxBuildError::InvalidPlan(
            "plan metadata does not match its StructureMap region".to_string(),
        ));
    }
    Ok(())
}

#[derive(Debug, Clone, Copy)]
struct TypedValue {
    id: WxValueId,
    ty: WxType,
}

#[derive(Debug, Clone)]
struct BlockSpec {
    id: WxBlockId,
    pc: usize,
    parameters: Vec<(Register, WxBlockParam)>,
}

#[derive(Debug, Clone)]
struct ExitBlockSpec {
    id: WxBlockId,
    exit: WxExitId,
    resume_pc: usize,
    parameters: Vec<(Register, WxBlockParam)>,
}

struct RegionBuilder<'a> {
    executable: &'a ExecutableFunction,
    plan: &'a JitPlan,
    leaders: HashSet<usize>,
    exit_by_pc: HashMap<usize, usize>,
    block_specs: HashMap<usize, BlockSpec>,
    exit_specs: HashMap<usize, ExitBlockSpec>,
    synthetic_exits: Vec<WxSideExit>,
    queue: VecDeque<usize>,
    built: HashSet<usize>,
    blocks: Vec<WxBlock>,
    next_value: u32,
    next_block: u32,
    next_exit: u32,
}
