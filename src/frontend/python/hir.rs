use super::SourceLocation;

#[derive(Debug)]
pub(crate) struct HirFunction {
    pub body: Vec<HirStatement>,
}

#[derive(Debug)]
pub(crate) struct HirStatement {
    pub kind: HirStatementKind,
    pub location: SourceLocation,
}

#[derive(Debug)]
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

#[derive(Debug)]
pub(crate) struct HirExpression {
    pub kind: HirExpressionKind,
    pub location: SourceLocation,
}

#[derive(Debug)]
pub(crate) enum HirExpressionKind {
    I64(i64),
    Name(String),
    Add(Box<HirExpression>, Box<HirExpression>),
    SignedLt(Box<HirExpression>, Box<HirExpression>),
}
