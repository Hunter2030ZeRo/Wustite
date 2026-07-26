pub type Register = u16;

pub enum Instruction {
    ConstI64 { dst: u16, value: i64 },
    AddI64 { dst: u16, lhs: u16, rhs: u16 },
    LtI64 { dst: u16, lhs: u16, rhs: u16 },
    Jump { target: usize },
    Branch { cond: u16, yes: usize, no: usize },
    Return { src: u16 },
}

pub struct Function {
    pub code: Vec<Instruction>,
    pub register_count: usize,
}
