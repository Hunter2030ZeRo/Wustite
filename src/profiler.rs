use crate::bytecode::{Instruction, Register};
use crate::object::ObjectHeap;
use crate::structure_map::{RegionId, SlotType, StateSlot, StructureMap};
use crate::value::Value;
use serde::{Deserialize, Serialize};

mod sequence;
#[cfg(test)]
mod tests;

use sequence::SiteSequenceProfile;
pub use sequence::{SequenceLayoutCase, SequenceSpecialization};

pub(crate) const READY_ENTRY_SAMPLES: u8 = 8;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ValueTag {
    SmallInt,
    Float,
    Bool,
    None,
    Object,
    Uninitialized,
}

impl ValueTag {
    pub const fn of(value: Value) -> Self {
        match value {
            Value::SmallInt(_) => Self::SmallInt,
            Value::Float(_) => Self::Float,
            Value::Bool(_) => Self::Bool,
            Value::None => Self::None,
            Value::Object(_) => Self::Object,
            Value::Uninitialized => Self::Uninitialized,
        }
    }
}

#[derive(Debug, Clone, Copy, Default)]
struct SiteResultProfile {
    first: Option<ValueTag>,
    second: Option<ValueTag>,
    megamorphic: bool,
    recent: Option<ValueTag>,
    recent_count: u8,
}

impl SiteResultProfile {
    const fn seeded(tag: ValueTag) -> Self {
        Self {
            first: Some(tag),
            second: None,
            megamorphic: false,
            recent: None,
            recent_count: 0,
        }
    }

    fn observe(&mut self, tag: ValueTag) {
        if self.recent == Some(tag) {
            self.recent_count = self.recent_count.saturating_add(1);
        } else {
            self.recent = Some(tag);
            self.recent_count = 1;
        }
        if self.megamorphic || self.first == Some(tag) || self.second == Some(tag) {
            return;
        }
        if self.first.is_none() {
            self.first = Some(tag);
        } else if self.second.is_none() {
            self.second = Some(tag);
        } else {
            self.megamorphic = true;
        }
    }

    const fn exact(self) -> Option<ValueTag> {
        if self.recent_count >= 2 {
            self.recent
        } else if self.megamorphic || self.second.is_some() {
            None
        } else {
            self.first
        }
    }

    const fn is_ready(self) -> bool {
        !self.megamorphic && self.recent.is_some() && self.recent_count >= READY_ENTRY_SAMPLES
    }
}

pub const PROFILE_ARTIFACT_VERSION: u32 = 1;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProfileArtifact {
    version: u32,
    fingerprint: String,
    regions: Vec<CachedRegionProfile>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct CachedRegionProfile {
    region: usize,
    tags: Vec<(Register, ValueTag)>,
}

impl ProfileArtifact {
    pub fn fingerprint(&self) -> &str {
        &self.fingerprint
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RegionProfileSchema {
    region_id: RegionId,
    probes: Vec<Register>,
    sequence_probes: Vec<Register>,
}

impl RegionProfileSchema {
    pub fn from_structure_map(structure_map: &StructureMap, region_id: RegionId) -> Option<Self> {
        let region = structure_map.region(region_id)?;
        let probes = region
            .entry_summary
            .iter()
            .filter(|slot| matches!(slot.ty, SlotType::Any))
            .map(|slot| slot.register)
            .collect();
        let sequence_probes = region
            .entry_summary
            .iter()
            .filter(|slot| {
                matches!(
                    slot.ty,
                    SlotType::Any
                        | SlotType::Object(crate::object::ObjectKind::List)
                        | SlotType::Object(crate::object::ObjectKind::Tuple)
                )
            })
            .map(|slot| slot.register)
            .collect();
        Some(Self {
            region_id,
            probes,
            sequence_probes,
        })
    }

    pub fn probes(&self) -> &[Register] {
        &self.probes
    }

    pub fn sequence_probes(&self) -> &[Register] {
        &self.sequence_probes
    }
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct ReadyRegionProfile<'a> {
    region_id: RegionId,
    profile: &'a Profile,
}

impl<'a> ReadyRegionProfile<'a> {
    pub(crate) const fn region_id(self) -> RegionId {
        self.region_id
    }

    pub(crate) const fn profile(self) -> &'a Profile {
        self.profile
    }
}

#[derive(Debug)]
pub struct Profile {
    region_entry_counts: Vec<u64>,
    region_entry_tags: Vec<Vec<(Register, SiteResultProfile)>>,
    region_entry_overrides: Vec<Vec<(Register, ValueTag)>>,
    region_entry_sequences: Vec<Vec<(Register, SiteSequenceProfile)>>,
    site_results: Vec<SiteResultProfile>,
    site_sequences: Vec<SiteSequenceProfile>,
}

impl Profile {
    pub fn new(region_count: usize, instruction_count: usize) -> Self {
        Self {
            region_entry_counts: vec![0; region_count],
            region_entry_tags: vec![Vec::new(); region_count],
            region_entry_overrides: vec![Vec::new(); region_count],
            region_entry_sequences: vec![Vec::new(); region_count],
            site_results: vec![SiteResultProfile::default(); instruction_count],
            site_sequences: vec![SiteSequenceProfile::default(); instruction_count],
        }
    }

    pub fn artifact(&self, fingerprint: String) -> ProfileArtifact {
        let regions = self
            .region_entry_tags
            .iter()
            .enumerate()
            .filter_map(|(region, profiles)| {
                let tags = profiles
                    .iter()
                    .filter_map(|(register, profile)| profile.exact().map(|tag| (*register, tag)))
                    .collect::<Vec<_>>();
                (!tags.is_empty()).then_some(CachedRegionProfile { region, tags })
            })
            .collect();
        ProfileArtifact {
            version: PROFILE_ARTIFACT_VERSION,
            fingerprint,
            regions,
        }
    }

    pub fn seed_from_artifact(
        &mut self,
        artifact: &ProfileArtifact,
        fingerprint: &str,
    ) -> Result<(), String> {
        if artifact.version != PROFILE_ARTIFACT_VERSION {
            return Err(format!(
                "profile artifact version {} is unsupported",
                artifact.version
            ));
        }
        if artifact.fingerprint != fingerprint {
            return Err("profile artifact fingerprint does not match executable".to_string());
        }
        for cached in &artifact.regions {
            let Some(profiles) = self.region_entry_tags.get_mut(cached.region) else {
                return Err(format!(
                    "profile artifact references region {}",
                    cached.region
                ));
            };
            for (register, tag) in &cached.tags {
                if profiles.iter().any(|(candidate, _)| candidate == register) {
                    continue;
                }
                profiles.push((*register, SiteResultProfile::seeded(*tag)));
            }
        }
        Ok(())
    }

    pub fn observe_entry(&mut self, region_id: RegionId, slots: &[StateSlot], registers: &[Value]) {
        self.observe_entry_registers(region_id, slots.iter().map(|slot| slot.register), registers);
    }

    pub fn observe_entry_schema(&mut self, schema: &RegionProfileSchema, registers: &[Value]) {
        self.observe_entry_registers(schema.region_id, schema.probes.iter().copied(), registers);
    }

    fn observe_entry_registers(
        &mut self,
        region_id: RegionId,
        registers_to_probe: impl IntoIterator<Item = Register>,
        registers: &[Value],
    ) {
        let Some(profiles) = self.region_entry_tags.get_mut(region_id.0) else {
            return;
        };
        for register in registers_to_probe {
            let Some(value) = registers.get(usize::from(register)).copied() else {
                continue;
            };
            let profile = if let Some((_, profile)) = profiles
                .iter_mut()
                .find(|(candidate, _)| *candidate == register)
            {
                profile
            } else {
                profiles.push((register, SiteResultProfile::default()));
                let Some((_, profile)) = profiles.last_mut() else {
                    continue;
                };
                profile
            };
            profile.observe(ValueTag::of(value));
        }
    }

    pub fn entry_tag(&self, region_id: RegionId, register: Register) -> Option<ValueTag> {
        if let Some(tag) = self
            .region_entry_overrides
            .get(region_id.0)?
            .iter()
            .find(|(candidate, _)| *candidate == register)
            .map(|(_, tag)| *tag)
        {
            return Some(tag);
        }
        self.region_entry_tags
            .get(region_id.0)?
            .iter()
            .find(|(candidate, _)| *candidate == register)
            .and_then(|(_, profile)| profile.exact())
    }

    pub fn observe_entry_sequences(
        &mut self,
        region_id: RegionId,
        slots: &[StateSlot],
        registers: &[Value],
        heap: &ObjectHeap,
    ) {
        self.observe_entry_sequence_registers(
            region_id,
            slots.iter().map(|slot| slot.register),
            registers,
            heap,
        );
    }

    pub fn observe_entry_sequences_schema(
        &mut self,
        schema: &RegionProfileSchema,
        registers: &[Value],
        heap: &ObjectHeap,
    ) {
        self.observe_entry_sequence_registers(
            schema.region_id,
            schema.sequence_probes.iter().copied(),
            registers,
            heap,
        );
    }

    fn observe_entry_sequence_registers(
        &mut self,
        region_id: RegionId,
        registers_to_probe: impl IntoIterator<Item = Register>,
        registers: &[Value],
        heap: &ObjectHeap,
    ) {
        let Some(profiles) = self.region_entry_sequences.get_mut(region_id.0) else {
            return;
        };
        for register in registers_to_probe {
            let Some(value) = registers.get(usize::from(register)).copied() else {
                continue;
            };
            let profile = if let Some((_, profile)) = profiles
                .iter_mut()
                .find(|(candidate, _)| *candidate == register)
            {
                profile
            } else {
                profiles.push((register, SiteSequenceProfile::default()));
                &mut profiles.last_mut().expect("entry just appended").1
            };
            profile.observe_value(value, heap);
        }
    }

    pub fn entry_sequence_specialization(
        &self,
        region_id: RegionId,
        register: Register,
    ) -> SequenceSpecialization {
        self.region_entry_sequences
            .get(region_id.0)
            .and_then(|profiles| {
                profiles
                    .iter()
                    .find(|(candidate, _)| *candidate == register)
            })
            .map_or(SequenceSpecialization::Unknown, |(_, profile)| {
                profile.specialization()
            })
    }

    pub fn observe_result(&mut self, pc: usize, value: Value) {
        if let Some(profile) = self.site_results.get_mut(pc) {
            profile.observe(ValueTag::of(value));
        }
    }

    pub fn observe_instruction(
        &mut self,
        pc: usize,
        instruction: &Instruction,
        registers: &[Value],
        heap: &crate::object::ObjectHeap,
    ) {
        if let Some(profile) = self.site_sequences.get_mut(pc) {
            profile.observe(instruction, registers, heap);
        }
        let Some(register) = result_register(instruction) else {
            return;
        };
        if let Some(value) = registers.get(usize::from(register)).copied() {
            self.observe_result(pc, value);
        }
    }

    pub fn sequence_specialization(&self, pc: usize) -> SequenceSpecialization {
        self.site_sequences
            .get(pc)
            .map_or(SequenceSpecialization::Unknown, |profile| {
                profile.specialization()
            })
    }

    pub fn result_tag(&self, pc: usize) -> Option<ValueTag> {
        self.site_results
            .get(pc)
            .and_then(|profile| profile.exact())
    }

    pub fn record_entry(&mut self, region_id: RegionId) {
        if let Some(count) = self.region_entry_counts.get_mut(region_id.0) {
            *count = count.saturating_add(1);
        }
    }

    pub fn entry_count(&self, region_id: RegionId) -> u64 {
        self.region_entry_counts
            .get(region_id.0)
            .copied()
            .unwrap_or(0)
    }

    pub fn is_hot(&self, region_id: RegionId, threshold: u64) -> bool {
        self.region_entry_counts
            .get(region_id.0)
            .is_some_and(|count| *count >= threshold)
    }

    pub(crate) fn ready_region(
        &self,
        structure_map: &StructureMap,
        region_id: RegionId,
        threshold: u64,
    ) -> Option<ReadyRegionProfile<'_>> {
        if !self.is_hot(region_id, threshold)
            || self.entry_count(region_id) < u64::from(READY_ENTRY_SAMPLES)
        {
            return None;
        }
        let schema = RegionProfileSchema::from_structure_map(structure_map, region_id)?;
        let profiles = self.region_entry_tags.get(region_id.0)?;
        let probes_ready = schema.probes().iter().all(|register| {
            profiles
                .iter()
                .find(|(candidate, _)| candidate == register)
                .is_some_and(|(_, profile)| profile.is_ready())
        });
        probes_ready.then_some(ReadyRegionProfile {
            region_id,
            profile: self,
        })
    }

    pub(crate) fn invalidate_region(&mut self, region_id: RegionId) {
        if let Some(tags) = self.region_entry_tags.get_mut(region_id.0) {
            tags.clear();
        }
        if let Some(overrides) = self.region_entry_overrides.get_mut(region_id.0) {
            overrides.clear();
        }
        if let Some(sequences) = self.region_entry_sequences.get_mut(region_id.0) {
            sequences.clear();
        }
        if let Some(count) = self.region_entry_counts.get_mut(region_id.0) {
            *count = 0;
        }
    }
}

const fn result_register(instruction: &Instruction) -> Option<Register> {
    match instruction {
        Instruction::ConstSmallInt { dst, .. }
        | Instruction::ConstFloat { dst, .. }
        | Instruction::ConstBool { dst, .. }
        | Instruction::ConstNone { dst }
        | Instruction::LoadConstant { dst, .. }
        | Instruction::ConstI64 { dst, .. }
        | Instruction::BinaryOp { dst, .. }
        | Instruction::CompareOp { dst, .. }
        | Instruction::UnaryOp { dst, .. }
        | Instruction::BooleanOp { dst, .. }
        | Instruction::BuildTuple { dst, .. }
        | Instruction::BuildList { dst, .. }
        | Instruction::BuildDict { dst, .. }
        | Instruction::GetItem { dst, .. }
        | Instruction::GetAttr { dst, .. }
        | Instruction::GetSlice { dst, .. }
        | Instruction::ListPop { dst, .. }
        | Instruction::Length { dst, .. }
        | Instruction::LoadCurrentFunction { dst }
        | Instruction::Call { dst, .. }
        | Instruction::CallMethod { dst, .. }
        | Instruction::AddI64 { dst, .. }
        | Instruction::LtI64 { dst, .. }
        | Instruction::Move { dst, .. } => Some(*dst),
        Instruction::SetItem { .. }
        | Instruction::SetAttr { .. }
        | Instruction::SetSlice { .. }
        | Instruction::ListAppend { .. }
        | Instruction::ListInsert { .. }
        | Instruction::Jump { .. }
        | Instruction::Branch { .. }
        | Instruction::Return { .. } => None,
    }
}
