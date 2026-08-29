mod control;
mod leaf;
mod liveness;
mod lowering;
mod operations;
mod runtime;
mod setup;
mod state;
mod virtuals;

use std::collections::{HashMap, HashSet, VecDeque};
use std::error::Error;
use std::fmt;

use crate::bytecode::{
    BinaryOperator, BooleanOperator, CompareOperator, Instruction, Register, UnaryOperator,
};
use crate::executable::ExecutableFunction;
use crate::object::SequenceStrategy;
use crate::planner::JitPlan;
use crate::profiler::{Profile, ReadyRegionProfile, SequenceSpecialization, ValueTag};
use crate::structure_map::{Fact, OperationSiteId, RegionKind, SlotType, StateSlot, TypeFact};
use crate::verifier as wvm_verifier;

use super::VerifiedWxFunction;
use super::ir::{
    WxBinaryOp, WxBlock, WxBlockId, WxBlockParam, WxBlockTarget, WxCastOp, WxCompareOp, WxConstant,
    WxExitId, WxExitKind, WxFloatBinaryOp, WxFloatCompareOp, WxFunction, WxGuardMode, WxInst,
    WxInstKind, WxInstResult, WxIntBinaryOp, WxIntCompareOp, WxIntOverflowOp, WxRegionOrigin,
    WxRuntimeInput, WxSequenceMutation, WxSideExit, WxStateValue, WxTerminator, WxValueId,
};
use super::types::{WxScalarType, WxType};

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
    UnsupportedLiveSlot {
        pc: usize,
        register: Register,
        actual: SlotType,
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
            Self::UnsupportedLiveSlot {
                pc,
                register,
                actual,
            } => write!(
                formatter,
                "cannot specialize live slot r{register} with actual SlotType::{actual:?} at pc {pc}"
            ),
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

impl WxBuildError {
    pub(crate) const fn live_slot_context(&self) -> Option<(Register, SlotType)> {
        match self {
            Self::UnsupportedLiveSlot {
                register, actual, ..
            } => Some((*register, *actual)),
            Self::InvalidExecutable(_)
            | Self::InvalidPlan(_)
            | Self::UnsupportedInstruction { .. }
            | Self::UnsupportedSpecialization { .. }
            | Self::MissingRegister { .. }
            | Self::TypeMismatch { .. }
            | Self::IdSpaceExhausted(_)
            | Self::InvalidWxir(_) => None,
        }
    }
}

/// Builds and verifies typed SSA for one planned WVM hot region.
pub fn build_region(
    executable: &ExecutableFunction,
    plan: &JitPlan,
) -> Result<WxFunction, WxBuildError> {
    build_verified_region(executable, plan).map(VerifiedWxFunction::into_function)
}

pub(crate) fn build_verified_region(
    executable: &ExecutableFunction,
    plan: &JitPlan,
) -> Result<VerifiedWxFunction, WxBuildError> {
    build_verified_region_with_profile(executable, plan, None)
}

pub(crate) fn build_profiled_region(
    executable: &ExecutableFunction,
    plan: &JitPlan,
    profile: ReadyRegionProfile<'_>,
) -> Result<VerifiedWxFunction, WxBuildError> {
    if profile.region_id() != plan.region_id {
        return Err(WxBuildError::InvalidPlan(
            "ready profile belongs to a different region".to_string(),
        ));
    }
    build_verified_region_with_profile(executable, plan, Some(profile.profile()))
}

fn build_verified_region_with_profile(
    executable: &ExecutableFunction,
    plan: &JitPlan,
    profile: Option<&Profile>,
) -> Result<VerifiedWxFunction, WxBuildError> {
    wvm_verifier::verify(executable).map_err(WxBuildError::InvalidExecutable)?;
    verify_plan(executable, plan)?;

    let mut builder = RegionBuilder::new(executable, plan, profile)?;
    let mut function = builder.build()?;
    super::optimizer::optimize(&mut function);
    VerifiedWxFunction::validate(function).map_err(WxBuildError::InvalidWxir)
}

fn verify_plan(executable: &ExecutableFunction, plan: &JitPlan) -> Result<(), WxBuildError> {
    let region = executable
        .structure_map()
        .region(plan.region_id)
        .ok_or_else(|| {
            WxBuildError::InvalidPlan(format!("unknown region ID {}", plan.region_id.0))
        })?;

    let RegionKind::Loop { backedge } = region.kind else {
        return Err(WxBuildError::InvalidPlan(
            "JIT plan must reference a loop region".to_string(),
        ));
    };

    if backedge != plan.backedge
        || region.entry != plan.header
        || region.exits != plan.exits
        || region.entry_summary != plan.live_slots
        || region.blocks != plan.blocks
        || region.summary != plan.summary
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
    profile: Option<&'a Profile>,
    leaders: HashSet<usize>,
    exit_by_pc: HashMap<usize, usize>,
    block_specs: HashMap<usize, BlockSpec>,
    exit_specs: HashMap<usize, ExitBlockSpec>,
    synthetic_exits: Vec<WxSideExit>,
    queue: VecDeque<usize>,
    built: HashSet<usize>,
    live_registers: HashMap<usize, HashSet<Register>>,
    pointer_registers: HashSet<Register>,
    blocks: Vec<WxBlock>,
    next_value: u32,
    next_block: u32,
    next_exit: u32,
}

const fn profiled_type(tag: ValueTag) -> Option<WxType> {
    match tag {
        ValueTag::SmallInt => Some(WxType::Scalar(WxScalarType::I64)),
        ValueTag::Float => Some(WxType::Scalar(WxScalarType::F64)),
        ValueTag::Bool => Some(WxType::Scalar(WxScalarType::I1)),
        ValueTag::Object => Some(WxType::Scalar(WxScalarType::RuntimeHandle)),
        ValueTag::None | ValueTag::Uninitialized => None,
    }
}
