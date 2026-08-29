use wustite::bytecode::{Function, Instruction};
use wustite::executable::{ExecutableFunction, ExecutableParameter};
use wustite::object::ObjectKind;
use wustite::planner::JitPlan;
use wustite::structure_map::{
    RegionExit, RegionId, RegionKind, SlotType, StateSlot, StructureMapBuilder,
};
use wustite::wxir::{WxInstKind, build_region};

#[test]
fn list_length_uses_an_explicit_sequence_instruction() {
    // Given: a loop whose live list is measured on every iteration.
    let function = Function {
        register_count: 3,
        code: vec![
            Instruction::Length { dst: 1, object: 0 },
            Instruction::ConstBool {
                dst: 2,
                value: true,
            },
            Instruction::Branch {
                cond: 2,
                yes: 3,
                no: 4,
            },
            Instruction::Jump { target: 0 },
            Instruction::Return { src: 1 },
        ],
    };
    let slots = vec![StateSlot {
        register: 0,
        ty: SlotType::Object(ObjectKind::List),
    }];
    let mut builder = StructureMapBuilder::new();
    builder
        .record_parameter(
            0,
            0,
            "values".to_string(),
            SlotType::Object(ObjectKind::List),
        )
        .unwrap();
    let region_id = builder.begin_region(0, slots.clone());
    builder
        .finish_region(
            region_id,
            RegionKind::Loop { backedge: 3 },
            vec![RegionExit { target: 4 }],
        )
        .unwrap();
    let structure_map = builder
        .finish(&function.code, function.register_count)
        .unwrap();
    let executable = ExecutableFunction::new_with_parameters(
        function,
        structure_map,
        vec![ExecutableParameter {
            name: "values".to_string(),
            register: 0,
            ty: SlotType::Object(ObjectKind::List),
        }],
    );
    let region = executable.structure_map().region(RegionId(0)).unwrap();

    // When: the loop is lowered to WXIR.
    let wxir = build_region(
        &executable,
        &JitPlan {
            region_id: RegionId(0),
            header: 0,
            backedge: 3,
            exits: vec![RegionExit { target: 4 }],
            live_slots: slots,
            blocks: region.blocks.clone(),
            summary: region.summary,
        },
    )
    .unwrap();

    // Then: length is explicit and no generic runtime call represents the access.
    let instructions = wxir
        .blocks
        .iter()
        .flat_map(|block| &block.instructions)
        .collect::<Vec<_>>();
    assert!(instructions.iter().any(|instruction| matches!(
        instruction.kind,
        WxInstKind::SequenceLength {
            pc: 0,
            object: 0,
            ..
        }
    )));
    assert!(
        !instructions
            .iter()
            .any(|instruction| matches!(instruction.kind, WxInstKind::RuntimeCall { pc: 0, .. }))
    );
}
