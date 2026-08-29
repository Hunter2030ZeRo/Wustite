use std::collections::BTreeMap;
use std::fmt;

use serde::{Deserialize, Serialize};

use super::profile::RecordPermit;
use super::wxir_v2::dependency::Dependency;
use super::wxir_v2::ir::{
    Block, BlockId, Constant, Effect, FactLowering, Instruction, InstructionKind, SnapshotDraft,
    Terminator, ValueDef, ValueId, ValueType,
};

mod recording;

pub(crate) const DEFAULT_TRACE_LIMIT: usize = 256;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub(crate) struct ExecutableIdentity {
    pub(crate) id: u64,
    pub(crate) epoch: u64,
}

impl ExecutableIdentity {
    pub(crate) const fn new(id: u64, epoch: u64) -> Self {
        Self { id, epoch }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub(crate) struct LoopPreheader {
    pub(crate) edge_pc: u32,
    pub(crate) body_pc: u32,
}

impl LoopPreheader {
    pub(crate) const fn matches(self, edge_pc: usize, body_pc: usize) -> bool {
        self.edge_pc as usize == edge_pc && self.body_pc as usize == body_pc
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub(crate) enum EntryKind {
    FunctionEntry,
    LoopHeader {
        header_pc: u32,
        backedge_pc: u32,
        preheader: Option<LoopPreheader>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct TraceStart {
    pub(crate) executable: ExecutableIdentity,
    pub(crate) entry: EntryKind,
    pub(crate) start_pc: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum TraceOp {
    Constant(Constant),
    Copy,
    IntegerAdd,
    IntegerSubtract,
    IntegerMultiply,
    IntegerCompare {
        comparison: super::wxir_v2::ir::NumericComparison,
    },
    FloatAdd,
    FloatSubtract,
    FloatMultiply,
    FloatDivide,
    FloatCompare {
        comparison: super::wxir_v2::ir::NumericComparison,
    },
    IntegerNegate,
    FloatNegate,
    BooleanNot,
    BooleanAnd,
    BooleanOr,
    ObjectGet,
    ObjectSet,
    ListGet,
    ListSet,
    ListAppend,
    Call {
        callee: u64,
    },
    Guard {
        guard: u32,
    },
    Allocate,
    Helper {
        helper: u64,
    },
    Branch {
        taken: bool,
        side_exit: u32,
    },
    NestedLoopHeader {
        header_pc: u32,
    },
    BorrowView,
    ResolveHandle,
    Fact {
        lowering: FactLowering,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TraceEvent {
    pc: u32,
    op: TraceOp,
    inputs: Vec<u16>,
    output: Option<(u16, ValueType)>,
    effect: Effect,
    safepoint: Option<super::wxir_v2::ir::SafepointId>,
}

impl TraceEvent {
    pub(crate) fn new(
        pc: u32,
        op: TraceOp,
        inputs: &[u16],
        output: Option<(u16, ValueType)>,
        effect: Effect,
    ) -> Self {
        Self {
            pc,
            op,
            inputs: inputs.to_vec(),
            output,
            effect,
            safepoint: None,
        }
    }

    pub(crate) fn effect_only(pc: u32, op: TraceOp) -> Self {
        Self::new(pc, op, &[], None, Effect::Pure)
    }

    pub(crate) const fn at_safepoint(mut self, point: super::wxir_v2::ir::SafepointId) -> Self {
        self.safepoint = Some(point);
        self
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum TraceError {
    ArbitraryPc { pc: u32 },
    MismatchedBackedge { expected: u32, actual: u32 },
    IrreducibleBackedge,
    TraceLimit { limit: usize },
    Terminated,
    MissingBackedge,
    UndefinedRegister { register: u16 },
    MissingSafepoint { pc: u32 },
}

impl fmt::Display for TraceError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "trace recording failed: {self:?}")
    }
}

impl std::error::Error for TraceError {}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RecordedTrace {
    executable: ExecutableIdentity,
    entry: EntryKind,
    schema_epoch: u64,
    instructions: Vec<Instruction>,
    parameters: Vec<ValueDef>,
    terminator: Terminator,
}

impl RecordedTrace {
    pub(crate) fn into_draft(self, dependencies: Vec<Dependency>) -> SnapshotDraft {
        SnapshotDraft::new(
            self.executable,
            self.entry,
            BlockId::new(0),
            vec![Block::new(
                BlockId::new(0),
                self.parameters,
                self.instructions,
                self.terminator,
            )],
            Vec::new(),
            Vec::new(),
            dependencies,
        )
        .with_schema_epoch(self.schema_epoch)
    }
}

#[derive(Debug)]
pub(crate) struct TraceRecorder {
    start: TraceStart,
    schema_epoch: u64,
    limit: usize,
    instructions: Vec<Instruction>,
    terminated: bool,
    registers: BTreeMap<u16, ValueId>,
    parameters: Vec<ValueDef>,
    next_value: u32,
}
