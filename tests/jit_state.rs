use wustite::bytecode::{Function, Instruction};
use wustite::executable::ExecutableFunction;
#[cfg(feature = "inkwell")]
use wustite::jit::LlvmRegionCompiler;
use wustite::jit::{CraneliftRegionCompiler, RegionCompiler};
use wustite::structure_map::{
    RegionExit, RegionId, RegionKind, SlotType, StateSlot, StructureMap, StructureMapBuilder,
};
use wustite::value::Value;
use wustite::wvm::Vm;
use wustite::wxir::{
    WxBlock, WxBlockId, WxBlockParam, WxExitId, WxExitKind, WxFunction, WxRegionOrigin,
    WxScalarType, WxSideExit, WxStateValue, WxTerminator, WxType, WxValueId,
};

fn executable_id_source() -> ExecutableFunction {
    ExecutableFunction::new(
        Function {
            code: vec![Instruction::Return { src: 0 }],
            register_count: 1,
        },
        StructureMap::default(),
    )
}

fn passthrough_function(ty: WxType) -> WxFunction {
    WxFunction {
        origin: WxRegionOrigin {
            region_id: RegionId(0),
            bytecode_header: 0,
            bytecode_backedge: 0,
        },
        entry: WxBlockId(0),
        entry_state: vec![WxStateValue {
            register: 0,
            value: WxValueId(0),
            ty,
        }],
        blocks: vec![WxBlock {
            id: WxBlockId(0),
            parameters: vec![WxBlockParam {
                id: WxValueId(0),
                ty,
            }],
            instructions: vec![],
            terminator: WxTerminator::SideExit {
                exit: WxExitId(0),
                values: vec![WxValueId(0)],
            },
        }],
        returns: vec![],
        side_exits: vec![WxSideExit {
            id: WxExitId(0),
            kind: WxExitKind::RegionExit,
            resume_pc: 1,
            state: vec![WxStateValue {
                register: 0,
                value: WxValueId(0),
                ty,
            }],
        }],
    }
}

fn f64_loop_function() -> ExecutableFunction {
    let function = Function {
        register_count: 5,
        code: vec![
            Instruction::ConstFloat {
                dst: 0,
                value: 42.25,
            },
            Instruction::ConstI64 { dst: 1, value: 0 },
            Instruction::ConstI64 { dst: 2, value: 1 },
            Instruction::ConstI64 { dst: 3, value: 2 },
            Instruction::LtI64 {
                dst: 4,
                lhs: 1,
                rhs: 3,
            },
            Instruction::Branch {
                cond: 4,
                yes: 6,
                no: 9,
            },
            Instruction::Move { dst: 0, src: 0 },
            Instruction::AddI64 {
                dst: 1,
                lhs: 1,
                rhs: 2,
            },
            Instruction::Jump { target: 4 },
            Instruction::Return { src: 0 },
        ],
    };
    let mut structure_map = StructureMapBuilder::new();
    let region = structure_map.begin_region(
        4,
        vec![
            StateSlot {
                register: 0,
                ty: SlotType::Float,
            },
            StateSlot {
                register: 1,
                ty: SlotType::SmallInt,
            },
            StateSlot {
                register: 2,
                ty: SlotType::SmallInt,
            },
            StateSlot {
                register: 3,
                ty: SlotType::SmallInt,
            },
        ],
    );
    structure_map
        .finish_region(
            region,
            RegionKind::Loop { backedge: 8 },
            vec![RegionExit { target: 9 }],
        )
        .unwrap();
    let structure_map = structure_map
        .finish(&function.code, function.register_count)
        .unwrap();
    ExecutableFunction::new(function, structure_map)
}

#[test]
fn f64_live_state_roundtrips_native_side_exit() {
    // Given: an F64 entry value with a noncanonical NaN payload.
    let executable = executable_id_source();
    let wxir = passthrough_function(WxType::Scalar(WxScalarType::F64));
    let mut compiler = CraneliftRegionCompiler::new(executable.id());
    let mut region = compiler.compile(&wxir).unwrap();
    let bits = 0x7ff8_0000_0000_0042;
    let mut registers = vec![Value::Float(f64::from_bits(bits))];

    // When: native code transports the value through its state buffer.
    let exit = region.execute(&mut registers).unwrap();

    // Then: the side exit and exact F64 bits are preserved.
    assert_eq!(exit.kind, WxExitKind::RegionExit);
    assert_eq!(exit.resume_pc, 1);
    let Value::Float(value) = registers[0] else {
        panic!("expected float state");
    };
    assert_eq!(value.to_bits(), bits);
}

#[test]
fn vm_executes_f64_live_state_in_native() {
    // Given: a loop with an F64 value live across its native region boundary.
    let executable = f64_loop_function();
    let mut vm = Vm::with_hot_threshold(0);
    vm.execute(&executable).unwrap();
    vm.execute(&executable).unwrap();

    // When: the adaptive VM executes the function.
    let result = vm.execute(&executable).unwrap();

    // Then: native execution preserves the float and compiles without fallback.
    assert_eq!(result.value, Value::Float(42.25));
    assert_eq!(vm.jit_report().compiled_regions, 1);
    assert_eq!(vm.jit_report().native_executions, 1);
    assert!(vm.jit_report().failures.is_empty());
}

#[cfg(feature = "inkwell")]
#[test]
fn llvm_f64_live_state_roundtrips_native_side_exit() {
    // Given: the same F64 state contract compiled directly by LLVM Tier-2.
    let executable = executable_id_source();
    let wxir = passthrough_function(WxType::Scalar(WxScalarType::F64));
    let mut compiler = LlvmRegionCompiler::new(executable.id());
    let mut region = compiler.compile(&wxir).unwrap();
    let bits = 0x7ff8_0000_0000_0042;
    let mut registers = vec![Value::Float(f64::from_bits(bits))];

    // When: LLVM transports the value through its state buffer.
    let exit = region.execute(&mut registers).unwrap();

    // Then: LLVM preserves the same side exit and exact F64 bits.
    assert_eq!(exit.kind, WxExitKind::RegionExit);
    assert_eq!(exit.resume_pc, 1);
    let Value::Float(value) = registers[0] else {
        panic!("expected float state");
    };
    assert_eq!(value.to_bits(), bits);
}
