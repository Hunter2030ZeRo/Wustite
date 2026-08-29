use std::collections::BTreeMap;

use crate::bytecode::Register;
use crate::wxir::{WxFunction, WxScalarType, WxStateValue, WxType};

use super::CompileError;

const ABI_WORD_SIZE: usize = size_of::<u64>();

/// One typed WVM register location in the native region state buffer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RegionSlot {
    pub register: Register,
    pub ty: WxType,
    pub offset: usize,
    pub size: usize,
    pub alignment: usize,
}

/// Stable byte layout shared by marshalling code and generated machine code.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RegionLayout {
    slots: Vec<RegionSlot>,
    size: usize,
    alignment: usize,
}

impl RegionLayout {
    pub(crate) fn new(function: &WxFunction) -> Result<Self, CompileError> {
        let mut register_types = BTreeMap::new();
        add_state_types(&mut register_types, &function.entry_state)?;
        for exit in &function.side_exits {
            add_state_types(&mut register_types, &exit.state)?;
        }

        let mut slots = Vec::with_capacity(register_types.len());
        for (index, (register, ty)) in register_types.into_iter().enumerate() {
            let (size, alignment) = type_layout(ty)?;
            slots.push(RegionSlot {
                register,
                ty,
                offset: index * ABI_WORD_SIZE,
                size,
                alignment,
            });
        }

        Ok(Self {
            size: slots.len() * ABI_WORD_SIZE,
            alignment: if slots.is_empty() { 1 } else { ABI_WORD_SIZE },
            slots,
        })
    }

    /// Returns all register slots in ascending WVM register order.
    pub fn slots(&self) -> &[RegionSlot] {
        &self.slots
    }

    /// Total native state-buffer size in bytes.
    pub fn size(&self) -> usize {
        self.size
    }

    /// Required native state-buffer alignment.
    pub fn alignment(&self) -> usize {
        self.alignment
    }

    pub(crate) fn slot(&self, register: Register) -> Result<RegionSlot, CompileError> {
        self.slots
            .iter()
            .find(|slot| slot.register == register)
            .copied()
            .ok_or_else(|| {
                CompileError::InvalidFunction(format!(
                    "register r{register} is absent from RegionLayout"
                ))
            })
    }

    pub(crate) fn word_count(&self) -> usize {
        self.size.div_ceil(ABI_WORD_SIZE)
    }

    pub(crate) fn word_index(&self, register: Register) -> Result<usize, CompileError> {
        Ok(self.slot(register)?.offset / ABI_WORD_SIZE)
    }
}

fn add_state_types(
    register_types: &mut BTreeMap<Register, WxType>,
    state: &[WxStateValue],
) -> Result<(), CompileError> {
    for value in state {
        if let Some(previous) = register_types.insert(value.register, value.ty)
            && previous != value.ty
        {
            return Err(CompileError::InvalidFunction(format!(
                "r{} has conflicting state types {} and {}",
                value.register, previous, value.ty
            )));
        }
    }
    Ok(())
}

fn type_layout(ty: WxType) -> Result<(usize, usize), CompileError> {
    match ty {
        WxType::Scalar(WxScalarType::I1) => Ok((1, 1)),
        WxType::Scalar(
            WxScalarType::I64 | WxScalarType::F64 | WxScalarType::RuntimeHandle | WxScalarType::Ptr,
        ) => Ok((8, 8)),
        _ => Err(CompileError::UnsupportedType(ty)),
    }
}
