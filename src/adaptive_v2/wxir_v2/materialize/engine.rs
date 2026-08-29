use std::collections::BTreeMap;

use crate::adaptive_v2::handles::StableHandle;
use crate::adaptive_v2::heap::BatchObject;
use crate::adaptive_v2::lists::ListStrategy;

use super::{
    DeoptEngine, DeoptError, MaterializedKind, MaterializedVirtual, ReconstructedFrame,
    ReconstructedState, RuntimeAtom,
};
use crate::adaptive_v2::wxir_v2::deopt::{DeoptRecipe, RegisterSource, VirtualKind, VirtualRecipe};
use crate::adaptive_v2::wxir_v2::dependency::DependencyKind;
use crate::adaptive_v2::wxir_v2::ir::Constant;

impl<'a> DeoptEngine<'a> {
    pub(crate) const fn new(
        heap: &'a crate::adaptive_v2::heap::GcHeap,
        values: &'a BTreeMap<crate::adaptive_v2::wxir_v2::ir::ValueId, RuntimeAtom>,
        spills: &'a BTreeMap<u32, RuntimeAtom>,
    ) -> Self {
        Self {
            heap,
            values,
            spills,
            shapes: None,
            forced_helper_failure: None,
        }
    }

    pub(crate) const fn with_shapes(
        mut self,
        shapes: &'a crate::adaptive_v2::shapes::ShapeTable,
    ) -> Self {
        self.shapes = Some(shapes);
        self
    }

    pub(crate) const fn with_forced_helper_failure(mut self, helper: u64) -> Self {
        self.forced_helper_failure = Some(helper);
        self
    }

    pub(crate) fn reconstruct(
        &self,
        recipe: &DeoptRecipe,
    ) -> Result<ReconstructedState, DeoptError> {
        if recipe
            .dependencies
            .iter()
            .any(|dependency| !dependency.is_current())
        {
            return Err(DeoptError::StaleDependency);
        }
        let required = [
            DependencyKind::Executable,
            DependencyKind::Schema,
            DependencyKind::GcAbi,
            DependencyKind::HelperAbi,
        ];
        if required.iter().any(|kind| {
            !recipe
                .dependencies
                .iter()
                .any(|dependency| dependency.kind == *kind)
        }) || !recipe.dependencies.iter().any(|dependency| {
            dependency.kind == DependencyKind::Executable
                && dependency.identity == recipe.executable.id
                && dependency.expected_epoch == recipe.executable.epoch
        }) {
            return Err(DeoptError::IncompleteDependency);
        }
        if let Some(helper) = self.forced_helper_failure {
            return Err(DeoptError::HelperFailure { helper });
        }
        let index = self.validate_virtuals(recipe)?;
        self.validate_frames(recipe, &index)?;
        let roots = self.external_roots(recipe)?;
        let batches = recipe
            .virtuals
            .iter()
            .map(|virtual_recipe| {
                let references = super::validate::virtual_sources(&virtual_recipe.kind)
                    .into_iter()
                    .filter_map(|source| self.batch_reference(source, &index).transpose())
                    .collect::<Result<Vec<_>, _>>()?;
                Ok(BatchObject::new(references))
            })
            .collect::<Result<Vec<_>, DeoptError>>()?;
        let handles = self.heap.allocate_graph_with_roots(&batches, &roots)?;
        let handle_map = recipe
            .virtuals
            .iter()
            .zip(handles.iter().copied())
            .map(|(virtual_recipe, handle)| (virtual_recipe.id, handle))
            .collect::<BTreeMap<_, _>>();
        let virtuals = recipe
            .virtuals
            .iter()
            .zip(handles)
            .map(|(virtual_recipe, handle)| {
                self.materialized_virtual(virtual_recipe, handle, &handle_map)
            })
            .collect::<Result<Vec<_>, _>>()?;
        let frames = recipe
            .frames
            .iter()
            .map(|frame| self.frame(frame, &handle_map))
            .collect::<Result<Vec<_>, _>>()?;
        Ok(ReconstructedState {
            resume_pc: recipe.resume_pc,
            mode: recipe.mode,
            frames,
            virtuals,
        })
    }

    fn materialized_virtual(
        &self,
        recipe: &VirtualRecipe,
        handle: StableHandle,
        handles: &BTreeMap<u32, StableHandle>,
    ) -> Result<MaterializedVirtual, DeoptError> {
        let kind = match &recipe.kind {
            VirtualKind::Object {
                shape_identity,
                shape_dependency_epoch,
                shape_layout_epoch,
                fields,
            } => MaterializedKind::Object {
                shape_identity: *shape_identity,
                shape_dependency_epoch: *shape_dependency_epoch,
                shape_layout_epoch: *shape_layout_epoch,
                fields: fields
                    .iter()
                    .map(|(symbol, source)| Ok((*symbol, self.resolve(source, handles)?)))
                    .collect::<Result<Vec<_>, DeoptError>>()?,
            },
            VirtualKind::List { items } => {
                let items = items
                    .iter()
                    .map(|source| self.resolve(source, handles))
                    .collect::<Result<Vec<_>, _>>()?;
                MaterializedKind::List {
                    strategy: list_strategy(&items),
                    items,
                }
            }
            VirtualKind::Tuple { items } => MaterializedKind::Tuple {
                items: items
                    .iter()
                    .map(|source| self.resolve(source, handles))
                    .collect::<Result<Vec<_>, _>>()?,
            },
        };
        Ok(MaterializedVirtual {
            id: recipe.id,
            handle,
            kind,
        })
    }

    fn frame(
        &self,
        recipe: &crate::adaptive_v2::wxir_v2::deopt::FrameRecipe,
        handles: &BTreeMap<u32, StableHandle>,
    ) -> Result<ReconstructedFrame, DeoptError> {
        let mut registers = recipe.registers.clone();
        registers.sort_by_key(|register| register.register);
        let registers = registers
            .iter()
            .map(|register| self.resolve(&register.source, handles))
            .collect::<Result<Vec<_>, _>>()?;
        Ok(ReconstructedFrame {
            function: recipe.function,
            resume_pc: recipe.resume_pc,
            registers,
            exception: recipe.exception.clone(),
        })
    }

    fn resolve(
        &self,
        source: &RegisterSource,
        handles: &BTreeMap<u32, StableHandle>,
    ) -> Result<RuntimeAtom, DeoptError> {
        match source {
            RegisterSource::Virtual(id) => handles
                .get(id)
                .copied()
                .map(RuntimeAtom::Handle)
                .ok_or(DeoptError::MissingVirtual { virtual_id: *id }),
            _ => self.resolve_external(source),
        }
    }

    pub(super) fn resolve_external(
        &self,
        source: &RegisterSource,
    ) -> Result<RuntimeAtom, DeoptError> {
        match source {
            RegisterSource::Ssa(value) => self
                .values
                .get(value)
                .copied()
                .ok_or(DeoptError::MissingValue { value: value.get() }),
            RegisterSource::Constant(Constant::HandleBits(_) | Constant::UndefinedDead) => {
                Err(DeoptError::InvalidConstant)
            }
            RegisterSource::Constant(constant) => Ok(constant_atom(constant)),
            RegisterSource::Spill { slot, .. } => self
                .spills
                .get(slot)
                .copied()
                .ok_or(DeoptError::MissingSpill { spill: *slot }),
            RegisterSource::UndefinedDead => Ok(RuntimeAtom::UndefinedDead),
            RegisterSource::Virtual(id) => Err(DeoptError::MissingVirtual { virtual_id: *id }),
        }
    }
}

fn constant_atom(constant: &Constant) -> RuntimeAtom {
    match constant {
        Constant::Integer(value) => RuntimeAtom::Integer(*value),
        Constant::FloatBits(bits) => RuntimeAtom::FloatBits(*bits),
        Constant::Boolean(value) => RuntimeAtom::Boolean(*value),
        Constant::HandleBits(_) | Constant::UndefinedDead => RuntimeAtom::UndefinedDead,
    }
}

fn list_strategy(items: &[RuntimeAtom]) -> ListStrategy {
    if items.is_empty() {
        return ListStrategy::Empty;
    }
    if items.iter().all(
        |item| matches!(item, RuntimeAtom::Integer(value) if (-70_368_744_177_664..=70_368_744_177_663).contains(value)),
    ) {
        return ListStrategy::ImmediateInteger;
    }
    if items
        .iter()
        .all(|item| matches!(item, RuntimeAtom::FloatBits(_)))
    {
        ListStrategy::F64
    } else {
        ListStrategy::Generic
    }
}
