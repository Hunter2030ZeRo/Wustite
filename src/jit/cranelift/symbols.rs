use std::collections::HashMap;

use crate::executable::ExecutableId;
use crate::structure_map::RegionId;

use super::CompileError;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct VersionId(u64);

#[derive(Debug)]
pub(in crate::jit) struct SymbolVersions {
    executable_id: ExecutableId,
    next_by_region: HashMap<RegionId, VersionId>,
}

impl SymbolVersions {
    pub(in crate::jit) fn new(executable_id: ExecutableId) -> Self {
        Self {
            executable_id,
            next_by_region: HashMap::new(),
        }
    }

    pub(in crate::jit) fn reserve(&mut self, region_id: RegionId) -> Result<String, CompileError> {
        let next = self.next_by_region.entry(region_id).or_insert(VersionId(0));
        let version = *next;
        next.0 = next.0.checked_add(1).ok_or_else(|| {
            CompileError::Backend(format!("region {} version space exhausted", region_id.0))
        })?;

        Ok(format!(
            "wustite_fn_{}_region_{}_v{}",
            self.executable_id.as_u64(),
            region_id.0,
            version.0
        ))
    }
}

#[cfg(test)]
mod tests {
    use crate::bytecode::Function;
    use crate::executable::ExecutableFunction;
    use crate::structure_map::{RegionId, StructureMap};

    use super::SymbolVersions;

    #[test]
    fn names_exact_versions_scoped_to_each_region() -> Result<(), super::CompileError> {
        let executable = ExecutableFunction::new(
            Function {
                code: Vec::new(),
                register_count: 0,
            },
            StructureMap::default(),
        );
        let executable_id = executable.id();
        let mut versions = SymbolVersions::new(executable_id);

        assert_eq!(
            versions.reserve(RegionId(4))?,
            format!("wustite_fn_{}_region_4_v0", executable_id.as_u64())
        );
        assert_eq!(
            versions.reserve(RegionId(9))?,
            format!("wustite_fn_{}_region_9_v0", executable_id.as_u64())
        );
        assert_eq!(
            versions.reserve(RegionId(4))?,
            format!("wustite_fn_{}_region_4_v1", executable_id.as_u64())
        );

        Ok(())
    }
}
