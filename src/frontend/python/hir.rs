use num_bigint::BigInt;

use super::SourceLocation;
use crate::bytecode::{BinaryOperator, BooleanOperator, CompareOperator, UnaryOperator};
use crate::executable::ExecutableFunction;
use crate::object::ClassObject;
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

#[derive(Clone)]
pub(crate) enum HirTarget {
    Name(String),
    Tuple(Vec<HirTarget>),
}

pub(crate) enum HirStatementKind {
    Assign {
        target: HirTarget,
        value: HirExpression,
    },
    SetItem {
        object: HirExpression,
        key: HirExpression,
        value: HirExpression,
    },
    SetAttr {
        object: HirExpression,
        name: String,
        value: HirExpression,
    },
    SetSlice {
        object: HirExpression,
        start: Option<HirExpression>,
        stop: Option<HirExpression>,
        step: Option<HirExpression>,
        value: HirExpression,
    },
    AugSetItem {
        object: HirExpression,
        key: HirExpression,
        op: BinaryOperator,
        value: HirExpression,
    },
    ListAppend {
        list: HirExpression,
        value: HirExpression,
    },
    ListInsert {
        list: HirExpression,
        index: HirExpression,
        value: HirExpression,
    },
    Expression(HirExpression),
    Break,
    While {
        condition: HirExpression,
        body: Vec<HirStatement>,
        orelse: Vec<HirStatement>,
    },
    If {
        condition: HirExpression,
        body: Vec<HirStatement>,
        orelse: Vec<HirStatement>,
    },
    ForRange {
        target: String,
        start: HirExpression,
        stop: HirExpression,
        step: i64,
        guaranteed_non_empty: bool,
        body: Vec<HirStatement>,
        orelse: Vec<HirStatement>,
    },
    ForSequence {
        targets: Vec<HirTarget>,
        iterables: Vec<HirExpression>,
        include_index: bool,
        body: Vec<HirStatement>,
        orelse: Vec<HirStatement>,
    },
    Return(HirExpression),
}

pub(crate) struct HirExpression {
    pub kind: HirExpressionKind,
    pub location: SourceLocation,
}

pub(crate) enum HirComprehensionIterator {
    Range {
        start: Box<HirExpression>,
        stop: Box<HirExpression>,
        step: i64,
    },
    Iterable(Box<HirExpression>),
}

pub(crate) enum HirExpressionKind {
    SmallInt(i64),
    Float(f64),
    Bool(bool),
    None,
    String(String),
    BigInt(BigInt),
    Function(Box<ExecutableFunction>),
    Class(Box<ClassObject>),
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
    ListComprehension {
        element: Box<HirExpression>,
        target: String,
        iterator: HirComprehensionIterator,
    },
    Dict(Vec<(HirExpression, HirExpression)>),
    GetItem {
        object: Box<HirExpression>,
        key: Box<HirExpression>,
    },
    GetAttr {
        object: Box<HirExpression>,
        name: String,
    },
    GetSlice {
        object: Box<HirExpression>,
        start: Option<Box<HirExpression>>,
        stop: Option<Box<HirExpression>>,
        step: Option<Box<HirExpression>>,
    },
    ListPop {
        list: Box<HirExpression>,
        index: Box<HirExpression>,
    },
    Length(Box<HirExpression>),
    Call {
        callable: Box<HirExpression>,
        args: Vec<HirExpression>,
    },
    CallMethod {
        receiver: Box<HirExpression>,
        name: String,
        args: Vec<HirExpression>,
    },
}
