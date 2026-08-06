use num_bigint::BigInt;

use super::SourceLocation;
use crate::bytecode::{BinaryOperator, BooleanOperator, CompareOperator, UnaryOperator};
use crate::executable::ExecutableFunction;
use crate::structure_map::SlotType;

pub(crate) struct HirFunction {
    pub parameters: Vec<HirParameter>,
    pub body: Vec<HirStatement>,
}

pub(crate) struct HirParameter {
    pub name: String,
    pub ty: SlotType,
    pub location: SourceLocation,
}

pub(crate) struct HirStatement {
    pub kind: HirStatementKind,
    pub location: SourceLocation,
}

pub(crate) enum HirStatementKind {
    Assign {
        name: String,
        value: HirExpression,
    },
    While {
        condition: HirExpression,
        body: Vec<HirStatement>,
    },
    Return(HirExpression),
}

pub(crate) struct HirExpression {
    pub kind: HirExpressionKind,
    pub location: SourceLocation,
}

pub(crate) enum HirExpressionKind {
    SmallInt(i64),
    Float(f64),
    Bool(bool),
    String(String),
    BigInt(BigInt),
    Function(Box<ExecutableFunction>),
    CurrentFunction,
    Name(String),
    Unary {
        op: UnaryOperator,
        operand: Box<HirExpression>,
    },
    Binary {
        op: BinaryOperator,
        lhs: Box<HirExpression>,
        rhs: Box<HirExpression>,
    },
    Compare {
        op: CompareOperator,
        lhs: Box<HirExpression>,
        rhs: Box<HirExpression>,
    },
    Boolean {
        op: BooleanOperator,
        values: Vec<HirExpression>,
    },
    Tuple(Vec<HirExpression>),
    List(Vec<HirExpression>),
    Dict(Vec<(HirExpression, HirExpression)>),
    GetItem {
        object: Box<HirExpression>,
        key: Box<HirExpression>,
    },
    Length(Box<HirExpression>),
    Call {
        callable: Box<HirExpression>,
        args: Vec<HirExpression>,
    },
}
