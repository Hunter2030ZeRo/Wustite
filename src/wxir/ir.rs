use std::fmt;

use crate::bytecode::Register;
use crate::structure_map::RegionId;

use super::types::WxType;

/// SSA value identifier. Each ID has exactly one definition in a function.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct WxValueId(pub u32);

/// Basic-block identifier local to a WXIR function.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct WxBlockId(pub u32);

/// Side-exit identifier local to a compiled region.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct WxExitId(pub u32);

impl fmt::Display for WxValueId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "v{}", self.0)
    }
}

impl fmt::Display for WxBlockId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "b{}", self.0)
    }
}

impl fmt::Display for WxExitId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "x{}", self.0)
    }
}

/// Links a WXIR region to its immutable WVM bytecode origin.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WxRegionOrigin {
    pub region_id: RegionId,
    pub bytecode_header: usize,
    pub bytecode_backedge: usize,
}

/// A typed SSA definition produced by an instruction.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WxInstResult {
    pub id: WxValueId,
    pub ty: WxType,
}

/// A typed value defined at basic-block entry instead of by a phi instruction.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WxBlockParam {
    pub id: WxValueId,
    pub ty: WxType,
}

/// Modular integer binary operations with one result.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WxIntBinaryOp {
    Add,
    Sub,
    Mul,
    And,
    Or,
    Xor,
}

/// Integer operations that explicitly produce a value and signed-overflow flag.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WxIntOverflowOp {
    Add,
}

/// Determines which boolean guard outcome leaves the compiled region.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WxGuardMode {
    ExitWhenTrue,
    ExitWhenFalse,
}

/// Floating-point binary operations.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WxFloatBinaryOp {
    Add,
    Sub,
    Mul,
    Div,
}

/// Type-directed binary operation family.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WxBinaryOp {
    Integer(WxIntBinaryOp),
    Float(WxFloatBinaryOp),
}

/// Integer comparisons.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WxIntCompareOp {
    Eq,
    Ne,
    SignedLt,
    SignedLe,
    UnsignedLt,
    UnsignedLe,
}

/// Floating-point comparisons.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WxFloatCompareOp {
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
}

/// Type-directed comparison family.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WxCompareOp {
    Integer(WxIntCompareOp),
    Float(WxFloatCompareOp),
}

/// Explicit scalar cast semantics.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WxCastOp {
    ZeroExtend,
    SignExtend,
    Truncate,
    IntToFloat { signed: bool },
    FloatToInt { signed: bool },
    FloatPromote,
    FloatDemote,
    PtrToInt,
    IntToPtr,
    Bitcast,
}

/// Literal constants. The result definition supplies the literal's type.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum WxConstant {
    Bool(bool),
    Int(i64),
    F32(f32),
    F64(f64),
    NullPtr,
}

/// Backend-independent instruction families.
#[derive(Debug, Clone, PartialEq)]
pub enum WxInstKind {
    Constant(WxConstant),
    Binary {
        op: WxBinaryOp,
        lhs: WxValueId,
        rhs: WxValueId,
    },
    IntegerBinaryWithOverflow {
        op: WxIntOverflowOp,
        lhs: WxValueId,
        rhs: WxValueId,
    },
    Compare {
        op: WxCompareOp,
        lhs: WxValueId,
        rhs: WxValueId,
    },
    Cast {
        op: WxCastOp,
        value: WxValueId,
    },
    Load {
        address: WxValueId,
    },
    Store {
        address: WxValueId,
        value: WxValueId,
    },
    PointerOffset {
        base: WxValueId,
        offset: WxValueId,
    },
    Splat {
        value: WxValueId,
    },
    ExtractLane {
        vector: WxValueId,
        lane: u16,
    },
    InsertLane {
        vector: WxValueId,
        lane: u16,
        value: WxValueId,
    },
    Shuffle {
        left: WxValueId,
        right: WxValueId,
        lanes: Vec<u16>,
    },
    Guard {
        condition: WxValueId,
        exit: WxExitId,
        mode: WxGuardMode,
    },
    Call {
        callee: String,
        arguments: Vec<WxValueId>,
        parameter_types: Vec<WxType>,
    },
}

/// One instruction and its zero or more SSA results.
#[derive(Debug, Clone, PartialEq)]
pub struct WxInst {
    pub results: Vec<WxInstResult>,
    pub kind: WxInstKind,
}

/// A control-flow edge and its block-parameter arguments.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WxBlockTarget {
    pub block: WxBlockId,
    pub arguments: Vec<WxValueId>,
}

/// Control flow is structurally separated from ordinary instructions.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WxTerminator {
    Jump {
        target: WxBlockId,
        arguments: Vec<WxValueId>,
    },
    Branch {
        condition: WxValueId,
        yes: WxBlockTarget,
        no: WxBlockTarget,
    },
    Return {
        values: Vec<WxValueId>,
    },
    SideExit {
        exit: WxExitId,
        values: Vec<WxValueId>,
    },
}

/// A basic block. The non-optional terminator makes unterminated blocks
/// unrepresentable.
#[derive(Debug, Clone, PartialEq)]
pub struct WxBlock {
    pub id: WxBlockId,
    pub parameters: Vec<WxBlockParam>,
    pub instructions: Vec<WxInst>,
    pub terminator: WxTerminator,
}

/// Maps one WVM register to an SSA value at a side exit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WxStateValue {
    pub register: Register,
    pub value: WxValueId,
    pub ty: WxType,
}

/// Metadata needed to resume WVM interpretation after native execution.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WxSideExit {
    pub id: WxExitId,
    pub resume_pc: usize,
    pub state: Vec<WxStateValue>,
}

/// A verified WXIR function representing a region or standalone routine.
#[derive(Debug, Clone, PartialEq)]
pub struct WxFunction {
    pub origin: WxRegionOrigin,
    pub entry: WxBlockId,
    /// Maps WVM registers to the entry block parameters used for marshalling.
    pub entry_state: Vec<WxStateValue>,
    pub blocks: Vec<WxBlock>,
    pub returns: Vec<WxType>,
    pub side_exits: Vec<WxSideExit>,
}
