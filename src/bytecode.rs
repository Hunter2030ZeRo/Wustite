pub type Register = u16;

#[derive(Clone, PartialEq, Eq)]
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
    Move {
        dst: Register,
        src: Register,
    },
}

#[derive(Clone, PartialEq, Eq)]
pub struct Function {
    pub code: Vec<Instruction>,
    pub register_count: usize,
}
