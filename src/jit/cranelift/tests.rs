use cranelift_module::Module;

use crate::bytecode::Function;
use crate::executable::{ExecutableFunction, ExecutableId};
use crate::structure_map::{RegionId, StructureMap};
use crate::wxir::{
    WxBlock, WxBlockId, WxExitId, WxExitKind, WxFunction, WxRegionOrigin, WxSideExit, WxTerminator,
};

use super::CraneliftRegionCompiler;
use crate::jit::{CompileError, RegionCompiler};

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

fn exiting_region(region_id: usize) -> WxFunction {
    let entry = WxBlockId(0);
    let exit = WxExitId(0);
    WxFunction {
        origin: WxRegionOrigin {
            region_id: RegionId(region_id),
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

fn returning_region(region_id: usize) -> WxFunction {
    let entry = WxBlockId(0);
    WxFunction {
        origin: WxRegionOrigin {
            region_id: RegionId(region_id),
            bytecode_header: 0,
            bytecode_backedge: 0,
        },
        entry,
        entry_state: Vec::new(),
        blocks: vec![WxBlock {
            id: entry,
            parameters: Vec::new(),
            instructions: Vec::new(),
            terminator: WxTerminator::Return { values: Vec::new() },
        }],
        returns: Vec::new(),
        side_exits: Vec::new(),
    }
}

#[test]
#[cfg_attr(miri, ignore = "Miri cannot execute Cranelift-generated native code")]
fn one_module_retains_multiple_regions_versions_and_old_entries() -> Result<(), String> {
    let executable_id = executable_id();
    let mut compiler = CraneliftRegionCompiler::new(executable_id);

    let mut first = compiler
        .compile(&exiting_region(0))
        .map_err(|error| error.to_string())?;
    compiler
        .compile(&exiting_region(1))
        .map_err(|error| error.to_string())?;
    compiler
        .compile(&exiting_region(0))
        .map_err(|error| error.to_string())?;

    let Some(module) = compiler.module.as_ref() else {
        return Err("module should be retained".to_string());
    };
    for symbol in [
        format!("wustite_fn_{}_region_0_v0", executable_id.as_u64()),
        format!("wustite_fn_{}_region_1_v0", executable_id.as_u64()),
        format!("wustite_fn_{}_region_0_v1", executable_id.as_u64()),
    ] {
        assert!(
            module.declarations().get_name(&symbol).is_some(),
            "missing declaration {symbol}"
        );
    }

    let exit = first.execute(&mut []).map_err(|error| error.to_string())?;
    assert_eq!(exit.kind, WxExitKind::RegionExit);

    Ok(())
}

#[test]
#[cfg_attr(miri, ignore = "Miri cannot execute Cranelift-generated native code")]
fn failed_compile_burns_its_region_version() -> Result<(), String> {
    let executable_id = executable_id();
    let mut compiler = CraneliftRegionCompiler::new(executable_id);

    let mut first = compiler
        .compile(&exiting_region(3))
        .map_err(|error| error.to_string())?;
    let error = match compiler.compile(&returning_region(3)) {
        Ok(_) => return Err("return lowering should be unsupported".to_string()),
        Err(error) => error,
    };
    assert!(matches!(
        error,
        CompileError::UnsupportedInstruction("Return")
    ));
    compiler
        .compile(&exiting_region(3))
        .map_err(|error| error.to_string())?;

    let Some(module) = compiler.module.as_ref() else {
        return Err("module should be retained".to_string());
    };
    let symbol = format!("wustite_fn_{}_region_3_v2", executable_id.as_u64());
    assert!(module.declarations().get_name(&symbol).is_some());

    let exit = first.execute(&mut []).map_err(|error| error.to_string())?;
    assert_eq!(exit.kind, WxExitKind::RegionExit);

    Ok(())
}

#[test]
fn public_compiler_rejects_unverified_wxir() {
    let mut function = exiting_region(0);
    function.blocks.push(function.blocks[0].clone());
    let mut compiler = CraneliftRegionCompiler::new(executable_id());

    assert!(matches!(
        compiler.compile(&function),
        Err(CompileError::InvalidFunction(_))
    ));
}
