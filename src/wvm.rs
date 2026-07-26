use crate::bytecode::Function;
use crate::value::Value;

pub struct Frame {
    pub pc: usize,
    pub registers: Vec<Value>,
}

pub struct Vm;

impl Vm {
    pub fn execute(&mut self, function: &Function) -> Result<Value, String> {
        todo!()
    }
}
