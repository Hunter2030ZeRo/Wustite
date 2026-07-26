use crate::structure_map::StructureMap;

pub type Register = u16;

pub enum Instruction {
    ConstI64 {
        dst: Register,
        value: i64,
    },
    AddI64 {
        dst: Register,
        lhs: Register,
        rhs: Register,
    },
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
}

pub struct Function {
    pub code: Vec<Instruction>,
    pub register_count: usize,
}

pub struct ExecutableFunction {
    pub bytecode: Function,
    pub structure_map: StructureMap,
}
