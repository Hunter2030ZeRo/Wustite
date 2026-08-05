use crate::structure_map::OperationSiteId;

pub type Register = u16;

/// Source-language binary operation preserved by the semantic WVM ISA.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BinaryOperator {
    Add,
}

/// Source-language comparison preserved by the semantic WVM ISA.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompareOperator {
    Lt,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Instruction {
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Function {
    pub code: Vec<Instruction>,
    pub register_count: usize,
}
