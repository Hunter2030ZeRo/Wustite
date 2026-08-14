use crate::bytecode::Function;
use crate::executable::{ExecutableFunction, ExecutableId};
use crate::jit::CraneliftRegionCompiler;
use crate::structure_map::{RegionId, StructureMap};

use super::{
    VerifiedWxFunction, WxBlock, WxBlockId, WxExitId, WxExitKind, WxFunction, WxRegionOrigin,
    WxSideExit, WxTerminator, reset_verification_count, verification_count,
};

fn executable_id() -> ExecutableId {
    ExecutableFunction::new(
        Function {
            code: Vec::new(),
            register_count: 0,
        },
        StructureMap::default(),
    )
    .id()
}

fn exiting_region() -> WxFunction {
    let entry = WxBlockId(0);
    let exit = WxExitId(0);
    WxFunction {
        origin: WxRegionOrigin {
            region_id: RegionId(0),
            bytecode_header: 0,
            bytecode_backedge: 0,
        },
        entry,
        entry_state: Vec::new(),
        blocks: vec![WxBlock {
            id: entry,
            parameters: Vec::new(),
            instructions: Vec::new(),
            terminator: WxTerminator::SideExit {
                exit,
                values: Vec::new(),
            },
        }],
        returns: Vec::new(),
        side_exits: vec![WxSideExit {
            id: exit,
            kind: WxExitKind::RegionExit,
            resume_pc: 0,
            state: Vec::new(),
        }],
    }
}

#[test]
#[cfg_attr(miri, ignore = "Miri cannot execute Cranelift-generated native code")]
fn internal_compilation_reuses_wxir_verification() -> Result<(), String> {
    reset_verification_count();
    let function = VerifiedWxFunction::validate(exiting_region())?;
    let mut compiler = CraneliftRegionCompiler::new(executable_id());

    let _region = compiler
        .compile_verified(&function)
        .map_err(|error| error.to_string())?;

    assert_eq!(verification_count(), 1);
    Ok(())
}
