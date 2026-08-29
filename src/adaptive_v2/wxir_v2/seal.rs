use super::ir::SnapshotDraft;
use super::{SnapshotError, SnapshotId, VerifiedSnapshot};
use crate::adaptive_v2::profile::CompilePermit;

pub(super) fn seal(
    mut draft: SnapshotDraft,
    permit: CompilePermit,
) -> Result<VerifiedSnapshot, SnapshotError> {
    if draft.body.schema_epoch != permit.schema_epoch() {
        return Err(SnapshotError::SchemaPermitMismatch {
            permit: permit.schema_epoch(),
            snapshot: draft.body.schema_epoch,
        });
    }
    draft.body.blocks.sort_by_key(|block| block.id);
    draft.body.root_maps.sort_by_key(|map| map.point);
    draft.body.deopts.sort_by_key(|recipe| recipe.id);
    for recipe in &mut draft.body.deopts {
        recipe
            .virtuals
            .sort_by_key(|virtual_recipe| virtual_recipe.id);
        recipe.dependencies.sort();
        recipe.explicit_roots.sort();
        recipe.explicit_roots.dedup();
        for frame in &mut recipe.frames {
            frame.registers.sort_by_key(|register| register.register);
        }
    }
    draft.body.dependencies.sort();
    super::verifier::verify(&draft.body)?;
    let canonical = serde_json::to_vec(&draft.body).map_err(|_| SnapshotError::Serialization)?;
    let id = SnapshotId(*blake3::hash(&canonical).as_bytes());
    Ok(VerifiedSnapshot::verified(id, draft.body))
}
