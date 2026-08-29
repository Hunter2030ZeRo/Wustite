use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

pub(crate) use super::snapshot::{SnapshotBody, SnapshotDraft};

macro_rules! id_type {
    ($name:ident) => {
        #[derive(
            Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
        )]
        pub(crate) struct $name(u32);

        impl $name {
            pub(crate) const fn new(value: u32) -> Self {
                Self(value)
            }

            pub(crate) const fn get(self) -> u32 {
                self.0
            }
        }
    };
}

id_type!(BlockId);
id_type!(ValueId);
id_type!(SafepointId);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub(crate) enum WxIrAbi {
    V2,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub(crate) enum ValueType {
    I64,
    F64,
    Bool,
    Handle,
    BorrowedView,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub(crate) enum NumericComparison {
    Equal,
    NotEqual,
    LessThan,
    LessEqual,
    GreaterThan,
    GreaterEqual,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub(crate) enum FactLowering {
    ElidedProven,
    GuardedStatic { guard: u32 },
    LiveProbe,
}

impl ValueType {
    pub(crate) const fn is_handle(self) -> bool {
        matches!(self, Self::Handle)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub(crate) enum Constant {
    Integer(i64),
    FloatBits(u64),
    Boolean(bool),
    HandleBits(u64),
    UndefinedDead,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub(crate) enum Effect {
    Pure,
    Read,
    Write,
    Allocation,
    Helper,
    Call,
    Backedge,
}

impl Effect {
    pub(crate) const fn is_barrier(self) -> bool {
        matches!(
            self,
            Self::Allocation | Self::Helper | Self::Call | Self::Backedge
        )
    }

    pub(crate) const fn is_ordered(self) -> bool {
        !matches!(self, Self::Pure | Self::Read)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub(crate) struct ValueDef {
    pub(crate) id: ValueId,
    pub(crate) ty: ValueType,
}

impl ValueDef {
    pub(crate) const fn new(id: ValueId, ty: ValueType) -> Self {
        Self { id, ty }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub(crate) enum InstructionKind {
    Constant(Constant),
    Copy,
    IntegerAdd,
    IntegerSubtract,
    IntegerMultiply,
    IntegerFloorDivide {
        divisor: i64,
    },
    IntegerToFloat,
    IntegerLessThan,
    IntegerCompare {
        comparison: NumericComparison,
    },
    FloatAdd,
    FloatSubtract,
    FloatMultiply,
    FloatDivide,
    FloatPower,
    FloatCompare {
        comparison: NumericComparison,
    },
    IntegerNegate,
    FloatNegate,
    BooleanNot,
    BooleanAnd,
    BooleanOr,
    Select,
    ObjectGet,
    ObjectSet,
    ListGet,
    ListLength,
    ListSet,
    ListReversePrefix {
        element_type: ValueType,
    },
    ListClear,
    ListAppend,
    ListInsert,
    ListPop,
    OwnedList {
        identity: u32,
        element_type: ValueType,
        reset_on_definition: bool,
        copy_from_source: bool,
    },
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
    BranchGuard {
        taken: bool,
        side_exit: u32,
    },
    NestedLoopExit {
        header_pc: u32,
    },
    BorrowView,
    ResolveHandle,
    LiveProbe,
    AtPc {
        pc: u32,
        operation: Box<InstructionKind>,
    },
}

impl InstructionKind {
    pub(crate) fn at_pc(self, pc: u32) -> Self {
        Self::AtPc {
            pc,
            operation: Box::new(self),
        }
    }

    pub(crate) fn semantic(&self) -> &Self {
        match self {
            Self::AtPc { operation, .. } => operation.semantic(),
            operation => operation,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub(crate) struct Instruction {
    pub(crate) kind: InstructionKind,
    pub(crate) inputs: Vec<ValueId>,
    pub(crate) output: Option<ValueDef>,
    pub(crate) effect: Effect,
    pub(crate) effect_sequence: Option<u32>,
    pub(crate) safepoint: Option<SafepointId>,
}

impl Instruction {
    pub(crate) const fn new(
        kind: InstructionKind,
        inputs: Vec<ValueId>,
        output: Option<ValueDef>,
        effect: Effect,
    ) -> Self {
        Self {
            kind,
            inputs,
            output,
            effect,
            effect_sequence: None,
            safepoint: None,
        }
    }

    pub(crate) const fn safepoint(
        kind: InstructionKind,
        inputs: Vec<ValueId>,
        output: Option<ValueDef>,
        effect: Effect,
        safepoint: SafepointId,
    ) -> Self {
        Self {
            kind,
            inputs,
            output,
            effect,
            effect_sequence: None,
            safepoint: Some(safepoint),
        }
    }

    pub(crate) const fn ordered(mut self, sequence: u32) -> Self {
        self.effect_sequence = Some(sequence);
        self
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub(crate) enum Terminator {
    Jump {
        target: BlockId,
        arguments: Vec<ValueId>,
    },
    Branch {
        condition: ValueId,
        yes: BlockId,
        no: BlockId,
    },
    Return {
        values: Vec<ValueId>,
    },
    SideExit {
        id: u32,
        values: Vec<ValueId>,
    },
    Backedge {
        target_pc: u32,
        safepoint: SafepointId,
    },
    IrreducibleBackedge,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub(crate) struct Block {
    pub(crate) id: BlockId,
    pub(crate) parameters: Vec<ValueDef>,
    pub(crate) instructions: Vec<Instruction>,
    pub(crate) terminator: Terminator,
}

impl Block {
    pub(crate) const fn new(
        id: BlockId,
        parameters: Vec<ValueDef>,
        instructions: Vec<Instruction>,
        terminator: Terminator,
    ) -> Self {
        Self {
            id,
            parameters,
            instructions,
            terminator,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub(crate) enum RootLocation {
    Ssa(ValueId),
    Spill(u32),
    Virtual(u32),
    OwnedList(u32),
    InlineRegister { frame: u16, register: u16 },
    CurrentFunction,
    Callee,
    Argument(u16),
    Result(u16),
    Cache(u32),
    DeoptWorklist,
    HostPin(u64),
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub(crate) struct RootMap {
    pub(crate) point: SafepointId,
    pub(crate) roots: BTreeSet<RootLocation>,
}

impl RootMap {
    pub(crate) const fn new(point: SafepointId, roots: BTreeSet<RootLocation>) -> Self {
        Self { point, roots }
    }
}
