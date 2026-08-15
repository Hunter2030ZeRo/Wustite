use crate::executable::ConstantId;
use crate::structure_map::OperationSiteId;

pub type Register = u16;

/// Source-language binary operation preserved by the semantic WVM ISA.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BinaryOperator {
    Add,
    Subtract,
    Multiply,
    Divide,
}

/// Source-language comparison preserved by the semantic WVM ISA.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompareOperator {
    Eq,
    NotEq,
    Lt,
    Le,
    Gt,
    Ge,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnaryOperator {
    Negate,
    Not,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BooleanOperator {
    And,
    Or,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Instruction {
    ConstSmallInt {
        dst: Register,
        value: i64,
    },
    ConstFloat {
        dst: Register,
        value: f64,
    },
    ConstBool {
        dst: Register,
        value: bool,
    },
    LoadConstant {
        dst: Register,
        constant: ConstantId,
    },
    ConstI64 {
        dst: Register,
        value: i64,
    },
    /// Generic source-language binary operation. The StructureMap operation
    /// site records facts that may justify a typed quickened or WXIR path.
    BinaryOp {
        dst: Register,
        op: BinaryOperator,
        lhs: Register,
        rhs: Register,
        site: OperationSiteId,
    },
    /// Generic source-language comparison with StructureMap-backed facts.
    CompareOp {
        dst: Register,
        op: CompareOperator,
        lhs: Register,
        rhs: Register,
        site: OperationSiteId,
    },
    UnaryOp {
        dst: Register,
        op: UnaryOperator,
        src: Register,
    },
    BooleanOp {
        dst: Register,
        op: BooleanOperator,
        lhs: Register,
        rhs: Register,
    },
    BuildTuple {
        dst: Register,
        items: Vec<Register>,
    },
    BuildList {
        dst: Register,
        items: Vec<Register>,
    },
    BuildDict {
        dst: Register,
        entries: Vec<(Register, Register)>,
    },
    GetItem {
        dst: Register,
        object: Register,
        key: Register,
    },
    SetItem {
        object: Register,
        key: Register,
        value: Register,
    },
    Length {
        dst: Register,
        object: Register,
    },
    LoadCurrentFunction {
        dst: Register,
    },
    Call {
        dst: Register,
        callable: Register,
        args: Vec<Register>,
    },
    /// Legacy typed opcode retained while existing hand-authored executables
    /// migrate to the semantic ISA. Frontends must not emit this variant.
    AddI64 {
        dst: Register,
        lhs: Register,
        rhs: Register,
    },
    /// Legacy typed opcode retained while existing hand-authored executables
    /// migrate to the semantic ISA. Frontends must not emit this variant.
    LtI64 {
        dst: Register,
        lhs: Register,
        rhs: Register,
    },
    Jump {
        target: usize,
    },
    Branch {
        cond: Register,
        yes: usize,
        no: usize,
    },
    Return {
        src: Register,
    },
    Move {
        dst: Register,
        src: Register,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub struct Function {
    pub code: Vec<Instruction>,
    pub register_count: usize,
}
