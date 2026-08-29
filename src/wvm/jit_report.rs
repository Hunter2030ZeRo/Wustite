use std::collections::HashMap;

use crate::bytecode::{Instruction, Register};
use crate::structure_map::{RegionId, SlotType};
use crate::wxir::{WxBuildError, WxExitKind};

/// Stage at which one region was disabled for the current execution.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JitFailureStage {
    BuildWxir,
    Compile,
    CompileTier2,
    Execute,
}

impl JitFailureStage {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::BuildWxir => "build_wxir",
            Self::Compile => "compile",
            Self::CompileTier2 => "compile_tier2",
            Self::Execute => "execute",
        }
    }
}

/// Preserved diagnostic for a failed synchronous tier-up operation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JitFailure {
    pub region_id: RegionId,
    pub stage: JitFailureStage,
    pub function: Option<String>,
    pub register: Option<Register>,
    pub actual_slot_type: Option<SlotType>,
    pub reason: String,
}

impl JitFailure {
    pub(crate) fn new(region_id: RegionId, stage: JitFailureStage, reason: String) -> Self {
        Self {
            region_id,
            stage,
            function: None,
            register: None,
            actual_slot_type: None,
            reason,
        }
    }

    pub(crate) fn from_wxir(region_id: RegionId, error: &WxBuildError) -> Self {
        let (register, actual_slot_type) = error
            .live_slot_context()
            .map_or((None, None), |(register, actual)| {
                (Some(register), Some(actual))
            });
        Self {
            region_id,
            stage: JitFailureStage::BuildWxir,
            function: None,
            register,
            actual_slot_type,
            reason: error.to_string(),
        }
    }

    pub(crate) fn at_function(mut self, function: Option<String>) -> Self {
        self.function = function;
        self
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct JitHelperCalls {
    pub call: u64,
    pub get_item: u64,
    pub set_item: u64,
    pub length: u64,
    pub object_access: u64,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct JitGuestCalls {
    pub direct_native: u64,
    pub interpreter_fallback: u64,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct JitExits {
    pub region_exit: u64,
    pub replay_instruction: u64,
    pub deopt: u64,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct JitCallSites {
    pub leaf_plans: u64,
    pub prepared_leaf_hit: u64,
    pub compiled_leaf_hit: u64,
    pub inlined_leaf: u64,
    pub prepared_call_hit: u64,
    pub call_guard_miss: u64,
    pub megamorphic_fallback: u64,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct JitRuntimeOps {
    pub load_constant: u64,
    pub binary: u64,
    pub compare: u64,
    pub unary: u64,
    pub boolean: u64,
    pub build_tuple: u64,
    pub build_list: u64,
    pub build_dict: u64,
    pub other: u64,
}

/// Observable tier-up activity for the latest execute invocation, including failures.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct JitReport {
    pub compilation_attempts: u64,
    pub compiled_regions: u64,
    pub tier2_compilation_attempts: u64,
    pub tier2_compiled_regions: u64,
    pub disabled_regions: u64,
    pub native_executions: u64,
    pub tier2_native_executions: u64,
    pub last_resume_pc: Option<usize>,
    pub last_exit_kind: Option<WxExitKind>,
    pub helper_calls: JitHelperCalls,
    pub guest_calls: JitGuestCalls,
    pub exits: JitExits,
    pub call_sites: JitCallSites,
    pub runtime_ops: JitRuntimeOps,
    pub calls: HashMap<String, u64>,
    pub native_calls: HashMap<String, u64>,
    pub failures: Vec<JitFailure>,
}

impl JitReport {
    pub const fn last_exit_kind_name(&self) -> Option<&'static str> {
        match self.last_exit_kind {
            Some(WxExitKind::RegionExit) => Some("region_exit"),
            Some(WxExitKind::ReplayInstruction) => Some("replay_instruction"),
            Some(WxExitKind::Deopt) => Some("deopt"),
            None => None,
        }
    }

    pub(super) fn record_native_helper(&mut self, instruction: &Instruction) {
        let counter = match instruction {
            Instruction::Call { .. } => {
                self.guest_calls.direct_native = self.guest_calls.direct_native.saturating_add(1);
                &mut self.helper_calls.call
            }
            Instruction::GetItem { .. } => &mut self.helper_calls.get_item,
            Instruction::SetItem { .. } => &mut self.helper_calls.set_item,
            Instruction::Length { .. } => &mut self.helper_calls.length,
            _ => &mut self.helper_calls.object_access,
        };
        *counter = counter.saturating_add(1);
        let operation = match instruction {
            Instruction::LoadConstant { .. } => &mut self.runtime_ops.load_constant,
            Instruction::BinaryOp { .. } | Instruction::AddI64 { .. } => {
                &mut self.runtime_ops.binary
            }
            Instruction::CompareOp { .. } | Instruction::LtI64 { .. } => {
                &mut self.runtime_ops.compare
            }
            Instruction::UnaryOp { .. } => &mut self.runtime_ops.unary,
            Instruction::BooleanOp { .. } => &mut self.runtime_ops.boolean,
            Instruction::BuildTuple { .. } => &mut self.runtime_ops.build_tuple,
            Instruction::BuildList { .. } => &mut self.runtime_ops.build_list,
            Instruction::BuildDict { .. } => &mut self.runtime_ops.build_dict,
            _ => &mut self.runtime_ops.other,
        };
        *operation = operation.saturating_add(1);
    }

    pub(super) fn record_interpreter_guest_call(&mut self) {
        self.guest_calls.interpreter_fallback =
            self.guest_calls.interpreter_fallback.saturating_add(1);
    }

    pub(super) fn record_exit(&mut self, kind: WxExitKind) {
        let counter = match kind {
            WxExitKind::RegionExit => &mut self.exits.region_exit,
            WxExitKind::ReplayInstruction => &mut self.exits.replay_instruction,
            WxExitKind::Deopt => &mut self.exits.deopt,
        };
        *counter = counter.saturating_add(1);
    }

    pub(super) fn record_function_call(&mut self, function: Option<&str>) {
        let count = self
            .calls
            .entry(function.unwrap_or("<anonymous>").to_owned())
            .or_default();
        *count = count.saturating_add(1);
    }

    pub(super) fn record_native_execution(&mut self, function: Option<&str>) {
        let count = self
            .native_calls
            .entry(function.unwrap_or("<anonymous>").to_owned())
            .or_default();
        *count = count.saturating_add(1);
    }
}
