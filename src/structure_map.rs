use crate::bytecode::{Instruction, Register};
use crate::object::ObjectKind;

mod facts;
mod region;
mod verify;

pub use facts::*;
pub use region::*;

/// Stable identifier for one semantic operation site in WVM bytecode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct OperationSiteId(pub u32);

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct StructureMap {
    blocks: Vec<BasicBlock>,
    regions: Vec<Region>,
    operation_sites: Vec<OperationSite>,
    values: Vec<ValueFact>,
    instructions: Vec<InstructionFact>,

    block_by_pc: Vec<BlockId>,
    region_by_entry_pc: Vec<Option<RegionId>>,
}

impl StructureMap {
    pub fn blocks(&self) -> &[BasicBlock] {
        &self.blocks
    }

    pub fn block(&self, id: BlockId) -> Option<&BasicBlock> {
        self.blocks.get(id.0 as usize)
    }

    pub fn region(&self, id: RegionId) -> Option<&Region> {
        self.regions.get(id.0)
    }

    pub fn regions(&self) -> &[Region] {
        &self.regions
    }

    pub fn operation_site(&self, id: OperationSiteId) -> Option<&OperationSite> {
        self.operation_sites.get(id.0 as usize)
    }

    pub fn operation_sites(&self) -> &[OperationSite] {
        &self.operation_sites
    }

    pub fn values(&self) -> &[ValueFact] {
        &self.values
    }

    pub fn value(&self, id: ValueId) -> Option<&ValueFact> {
        self.values.get(id.0 as usize)
    }

    pub fn instruction_facts(&self) -> &[InstructionFact] {
        &self.instructions
    }

    pub fn instruction_fact(&self, pc: usize) -> Option<&InstructionFact> {
        self.instructions.get(pc)
    }

    pub fn same_identity(&self, lhs: ValueId, rhs: ValueId) -> bool {
        matches!(
            (self.identity_root(lhs), self.identity_root(rhs)),
            (Some(lhs), Some(rhs)) if lhs == rhs
        )
    }

    fn identity_root(&self, mut id: ValueId) -> Option<ValueId> {
        for _ in 0..=self.values.len() {
            let value = self.value(id)?;
            match value.identity {
                Fact::Proven(IdentityFact::AliasOf(source)) => id = source,
                Fact::Proven(IdentityFact::Fresh)
                | Fact::Proven(IdentityFact::Unknown)
                | Fact::Guardable(_)
                | Fact::Unknown => return Some(id),
            }
        }
        None
    }

    pub fn block_by_pc(&self, pc: usize) -> Option<&BasicBlock> {
        self.block_by_pc.get(pc).and_then(|id| self.block(*id))
    }

    pub fn region_by_entry_pc(&self, pc: usize) -> Option<RegionId> {
        self.region_by_entry_pc.get(pc).copied().flatten()
    }

    pub fn loop_regions(&self) -> impl Iterator<Item = (RegionId, &Region)> {
        self.regions
            .iter()
            .enumerate()
            .filter(|(_, region)| matches!(region.kind, RegionKind::Loop { .. }))
            .map(|(id, region)| (RegionId(id), region))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ParameterSeed {
    register: Register,
    index: usize,
    name: String,
    ty: SlotType,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ConstantSeed {
    index: usize,
    kind: ObjectKind,
}

#[derive(Default, Debug, Clone, PartialEq, Eq)]
pub struct StructureMapBuilder {
    operation_sites: Vec<OperationSite>,
    regions: Vec<RegionDraft>,
    parameters: Vec<ParameterSeed>,
    constants: Vec<ConstantSeed>,
}

impl StructureMapBuilder {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn record_operation(
        &mut self,
        pc: usize,
        lhs: TypeFact,
        rhs: TypeFact,
        result: TypeFact,
    ) -> Result<OperationSiteId, String> {
        let id = u32::try_from(self.operation_sites.len())
            .map_err(|_| "StructureMap contains too many operation sites".to_string())?;
        self.operation_sites.push(OperationSite {
            pc,
            lhs,
            rhs,
            result,
        });
        Ok(OperationSiteId(id))
    }

    pub fn record_parameter(
        &mut self,
        register: Register,
        index: usize,
        name: String,
        ty: SlotType,
    ) -> Result<(), String> {
        if self
            .parameters
            .iter()
            .any(|parameter| parameter.register == register || parameter.index == index)
        {
            return Err(format!(
                "duplicate parameter origin for index {index} or r{register}"
            ));
        }
        self.parameters.push(ParameterSeed {
            register,
            index,
            name,
            ty,
        });
        Ok(())
    }

    pub fn record_constant(&mut self, index: usize, kind: ObjectKind) -> Result<(), String> {
        if self
            .constants
            .iter()
            .any(|constant| constant.index == index)
        {
            return Err(format!("duplicate constant origin for index {index}"));
        }
        self.constants.push(ConstantSeed { index, kind });
        Ok(())
    }

    pub fn begin_region(&mut self, entry: usize, entry_summary: Vec<StateSlot>) -> RegionId {
        let id = RegionId(self.regions.len());
        self.regions.push(RegionDraft {
            entry,
            entry_summary,
            completion: None,
        });
        id
    }

    pub fn finish_region(
        &mut self,
        id: RegionId,
        kind: RegionKind,
        exits: Vec<RegionExit>,
    ) -> Result<(), String> {
        let draft = self
            .regions
            .get_mut(id.0)
            .ok_or_else(|| format!("unknown region {}", id.0))?;
        if draft.completion.is_some() {
            return Err(format!("region {} is already finished", id.0));
        }
        draft.completion = Some((kind, exits));
        Ok(())
    }

    pub fn update_region_entry_summary(
        &mut self,
        id: RegionId,
        entry_summary: Vec<StateSlot>,
    ) -> Result<(), String> {
        let draft = self
            .regions
            .get_mut(id.0)
            .ok_or_else(|| format!("unknown region {}", id.0))?;
        if draft.completion.is_some() {
            return Err(format!("region {} is already finished", id.0));
        }
        draft.entry_summary = entry_summary;
        Ok(())
    }

    pub fn finish(
        self,
        code: &[Instruction],
        register_count: usize,
    ) -> Result<StructureMap, String> {
        builder::finish(self, code, register_count)
    }
}

mod builder;

#[cfg(test)]
mod tests;

#[cfg(test)]
mod analysis_tests;
