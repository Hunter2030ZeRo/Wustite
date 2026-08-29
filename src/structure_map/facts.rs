use crate::bytecode::Register;
use crate::object::ObjectKind;

mod sequence;

pub use sequence::*;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SlotType {
    SmallInt,
    Float,
    Bool,
    Object(ObjectKind),
    Any,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Fact<T> {
    Proven(T),
    Guardable(T),
    Unknown,
}

impl<T> Fact<T> {
    pub const fn as_ref(&self) -> Fact<&T> {
        match self {
            Self::Proven(value) => Fact::Proven(value),
            Self::Guardable(value) => Fact::Guardable(value),
            Self::Unknown => Fact::Unknown,
        }
    }

    pub const fn proven(&self) -> Option<&T> {
        match self {
            Self::Proven(value) => Some(value),
            Self::Guardable(_) | Self::Unknown => None,
        }
    }

    pub const fn candidate(&self) -> Option<&T> {
        match self {
            Self::Proven(value) | Self::Guardable(value) => Some(value),
            Self::Unknown => None,
        }
    }
}

pub type TypeFact = Fact<SlotType>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OperationSite {
    pub pc: usize,
    pub lhs: TypeFact,
    pub rhs: TypeFact,
    pub result: TypeFact,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ValueId(pub u32);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ValueUse {
    pub register: Register,
    pub value: Option<ValueId>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ValueOrigin {
    Parameter {
        index: usize,
        name: String,
    },
    Immediate {
        pc: usize,
    },
    ConstantPool {
        pc: usize,
        index: usize,
        kind: Option<ObjectKind>,
    },
    CurrentFunction {
        pc: usize,
    },
    Allocation {
        pc: usize,
        kind: ObjectKind,
    },
    Operation {
        pc: usize,
    },
    Projection {
        pc: usize,
        aggregate: ValueUse,
    },
    Call {
        pc: usize,
        callable: ValueUse,
    },
    Alias {
        pc: usize,
        source: ValueUse,
    },
    Unknown {
        pc: usize,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ValueComposition {
    None,
    Sequence(Vec<ValueUse>),
    Mapping(Vec<(ValueUse, ValueUse)>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IdentityFact {
    Fresh,
    AliasOf(ValueId),
    Unknown,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord)]
pub enum EscapeState {
    #[default]
    Local,
    Region,
    Function,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FailureKind {
    Arithmetic,
    DivisionByZero,
    Type,
    Index,
    Key,
    Value,
    Call,
    Allocation,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct EffectSummary {
    pub may_mutate: bool,
    pub may_allocate: bool,
    pub may_call_unknown: bool,
    pub may_access_global_state: bool,
}

impl EffectSummary {
    pub const fn is_pure(self) -> bool {
        !self.may_mutate
            && !self.may_allocate
            && !self.may_call_unknown
            && !self.may_access_global_state
    }

    pub fn include(&mut self, other: Self) {
        self.may_mutate |= other.may_mutate;
        self.may_allocate |= other.may_allocate;
        self.may_call_unknown |= other.may_call_unknown;
        self.may_access_global_state |= other.may_access_global_state;
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ControlDependency {
    pub branch_pc: usize,
    pub condition: ValueUse,
    pub expected: bool,
    pub hoistable: Fact<bool>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValueFact {
    pub id: ValueId,
    pub register: Register,
    pub defined_at: Option<usize>,
    pub origin: Fact<ValueOrigin>,
    pub ty: TypeFact,
    pub identity: Fact<IdentityFact>,
    pub composition: Fact<ValueComposition>,
    pub escape: Fact<EscapeState>,
    pub sequence: SequenceFacts,
}

impl ValueFact {
    pub fn is_virtualizable(&self) -> bool {
        matches!(self.origin, Fact::Proven(ValueOrigin::Allocation { .. }))
            && self.escape == Fact::Proven(EscapeState::Local)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstructionFact {
    pub pc: usize,
    pub inputs: Vec<ValueUse>,
    pub output: Option<ValueId>,
    pub effects: Fact<EffectSummary>,
    pub mutated_values: Fact<Vec<ValueId>>,
    pub mutations: Fact<Vec<MutationEffect>>,
    pub failures: Fact<Vec<FailureKind>>,
    pub guard_placement: Fact<GuardPlacement>,
    pub control_dependencies: Vec<ControlDependency>,
}
