use wustite::bytecode::{Function, Instruction};
use wustite::executable::{
    ConstantId, ExecutableConstant, ExecutableFunction, ExecutableParameter,
};
use wustite::planner::JitPlan;
use wustite::structure_map::{RegionId, RegionKind, SlotType, StateSlot, StructureMapBuilder};
use wustite::wxir::{self, WxInstKind, WxScalarType, WxTerminator, WxType, build_region};

#[test]
fn load_constant_becomes_native_runtime_call() {
    let function = Function {
        register_count: 2,
        code: vec![
            Instruction::LoadConstant {
                dst: 1,
                constant: ConstantId(0),
            },
            Instruction::Jump { target: 0 },
        ],
    };
    let mut structure_map = StructureMapBuilder::new();
    let region = structure_map.begin_region(
        0,
        vec![StateSlot {
            register: 0,
            ty: SlotType::SmallInt,
        }],
    );
    structure_map
        .finish_region(region, RegionKind::Loop { backedge: 1 }, vec![])
        .unwrap();
    let structure_map = structure_map
        .finish(&function.code, function.register_count)
        .unwrap();
    let executable = ExecutableFunction::new_with_abi(
        function,
        structure_map,
        vec![ExecutableParameter {
            name: "value".to_owned(),
            register: 0,
            ty: SlotType::SmallInt,
        }],
        vec![ExecutableConstant::String("constant".to_owned())],
    );
    let region = executable.structure_map().region(RegionId(0)).unwrap();
    let plan = JitPlan {
        region_id: RegionId(0),
        header: 0,
        backedge: 1,
        exits: vec![],
        live_slots: region.entry_summary.clone(),
        blocks: region.blocks.clone(),
        summary: region.summary,
    };

    // Given a loop whose first instruction loads an object constant,
    // When WXIR lowers the region,
    let function = build_region(&executable, &plan).unwrap();

    // Then native code preserves live state and dispatches the constant load in-region.
    wxir::verify(&function).unwrap();
    let entry = function
        .blocks
        .iter()
        .find(|block| block.id == function.entry)
        .unwrap();
    assert!(matches!(
        entry.instructions.as_slice(),
        [instruction]
            if matches!(
                &instruction.kind,
                WxInstKind::RuntimeCall {
                    pc: 0,
                    inputs,
                    output: Some(1),
                    ..
                }
                    if inputs.is_empty()
            ) && instruction.results[0].ty == WxType::Scalar(WxScalarType::RuntimeHandle)
    ));
    assert!(matches!(entry.terminator, WxTerminator::Jump { .. }));
    assert!(function.side_exits.is_empty());
}
