use std::collections::{BTreeMap, BTreeSet};

use crate::adaptive_v2::heap::BatchReference;
use crate::adaptive_v2::roots::{RootInventory, RootKind};

use super::{DeoptEngine, DeoptError, RuntimeAtom};
use crate::adaptive_v2::wxir_v2::deopt::{DeoptRecipe, RegisterSource, VirtualKind};
use crate::adaptive_v2::wxir_v2::ir::ValueType;

impl DeoptEngine<'_> {
    pub(super) fn validate_virtuals(
        &self,
        recipe: &DeoptRecipe,
    ) -> Result<BTreeMap<u32, usize>, DeoptError> {
        let mut index = BTreeMap::new();
        for (position, virtual_recipe) in recipe.virtuals.iter().enumerate() {
            if index.insert(virtual_recipe.id, position).is_some() {
                return Err(DeoptError::DuplicateVirtual {
                    virtual_id: virtual_recipe.id,
                });
            }
        }
        for virtual_recipe in &recipe.virtuals {
            if let VirtualKind::Object {
                shape_identity,
                shape_dependency_epoch,
                shape_layout_epoch,
                ..
            } = virtual_recipe.kind
            {
                let current = self.shapes.is_some_and(|shapes| {
                    shapes.serialized_key_is_current(
                        shape_identity,
                        shape_dependency_epoch,
                        shape_layout_epoch,
                    )
                });
                if !current {
                    return Err(DeoptError::StaleShape {
                        shape: shape_identity,
                    });
                }
            }
            for source in virtual_sources(&virtual_recipe.kind) {
                if let RegisterSource::Virtual(id) = source {
                    if !index.contains_key(id) {
                        return Err(DeoptError::MissingVirtual { virtual_id: *id });
                    }
                } else {
                    let atom = self.resolve_external(source)?;
                    if let RegisterSource::Spill { ty, .. } = source
                        && atom.ty() != Some(*ty)
                    {
                        return Err(DeoptError::TypeMismatch { register: u16::MAX });
                    }
                }
            }
        }
        Ok(index)
    }

    pub(super) fn validate_frames(
        &self,
        recipe: &DeoptRecipe,
        virtuals: &BTreeMap<u32, usize>,
    ) -> Result<(), DeoptError> {
        for frame in &recipe.frames {
            let mut registers = frame.registers.iter().collect::<Vec<_>>();
            registers.sort_by_key(|register| register.register);
            if registers
                .iter()
                .enumerate()
                .any(|(index, register)| usize::from(register.register) != index)
            {
                return Err(DeoptError::NonContiguousRegisters {
                    function: frame.function,
                });
            }
            let undefined = registers
                .iter()
                .filter(|register| matches!(register.source, RegisterSource::UndefinedDead))
                .map(|register| register.register)
                .collect::<BTreeSet<_>>();
            if undefined != frame.dead_registers {
                return Err(DeoptError::NonContiguousRegisters {
                    function: frame.function,
                });
            }
            for register in registers {
                if let RegisterSource::Spill { ty, .. } = register.source
                    && ty != register.ty
                {
                    return Err(DeoptError::TypeMismatch {
                        register: register.register,
                    });
                }
                let atom = match register.source {
                    RegisterSource::Virtual(id) => {
                        if !virtuals.contains_key(&id) {
                            return Err(DeoptError::MissingVirtual { virtual_id: id });
                        }
                        RuntimeAtom::UndefinedDead
                    }
                    _ => self.resolve_external(&register.source)?,
                };
                if !matches!(register.source, RegisterSource::Virtual(_))
                    && atom.ty() != Some(register.ty)
                    && atom != RuntimeAtom::UndefinedDead
                {
                    return Err(DeoptError::TypeMismatch {
                        register: register.register,
                    });
                }
                if matches!(register.source, RegisterSource::Virtual(_))
                    && register.ty != ValueType::Handle
                {
                    return Err(DeoptError::TypeMismatch {
                        register: register.register,
                    });
                }
            }
        }
        Ok(())
    }

    pub(super) fn external_roots(&self, recipe: &DeoptRecipe) -> Result<RootInventory, DeoptError> {
        let mut roots = RootInventory::new();
        let mut seen = BTreeSet::new();
        for atom in self.values.values().chain(self.spills.values()) {
            if let RuntimeAtom::Handle(handle) = atom {
                self.heap.resolve(*handle)?;
                if seen.insert((handle.slot(), handle.generation())) {
                    roots.insert(RootKind::DeoptMaterialization, *handle);
                }
            }
        }
        for frame in &recipe.frames {
            for register in &frame.registers {
                if !matches!(register.source, RegisterSource::Virtual(_)) {
                    let atom = self.resolve_external(&register.source)?;
                    if atom.ty() != Some(register.ty) && atom != RuntimeAtom::UndefinedDead {
                        return Err(DeoptError::TypeMismatch {
                            register: register.register,
                        });
                    }
                }
            }
        }
        Ok(roots)
    }

    pub(super) fn batch_reference(
        &self,
        source: &RegisterSource,
        virtuals: &BTreeMap<u32, usize>,
    ) -> Result<Option<BatchReference>, DeoptError> {
        match source {
            RegisterSource::Virtual(id) => virtuals
                .get(id)
                .copied()
                .map(BatchReference::Object)
                .map(Some)
                .ok_or(DeoptError::MissingVirtual { virtual_id: *id }),
            _ => match self.resolve_external(source)? {
                RuntimeAtom::Handle(handle) => Ok(Some(BatchReference::External(handle))),
                RuntimeAtom::Integer(_)
                | RuntimeAtom::FloatBits(_)
                | RuntimeAtom::Boolean(_)
                | RuntimeAtom::UndefinedDead => Ok(None),
            },
        }
    }
}

pub(super) fn virtual_sources(kind: &VirtualKind) -> Vec<&RegisterSource> {
    match kind {
        VirtualKind::Object { fields, .. } => fields.iter().map(|(_, source)| source).collect(),
        VirtualKind::List { items } | VirtualKind::Tuple { items } => items.iter().collect(),
    }
}
