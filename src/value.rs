#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Value {
    I64(i64),
    Bool(bool),
    Uninitialized,
}
