use serde::Serialize;
use wustite::ExecutableInfo;

use super::value_names::slot_type_name;

#[derive(Serialize)]
pub(super) struct InspectDocument {
    path: String,
    function: String,
    register_count: usize,
    instruction_count: usize,
    parameters: Vec<ParameterOutput>,
    regions: Vec<RegionOutput>,
}

impl InspectDocument {
    pub(super) fn new(path: String, function: String, info: ExecutableInfo) -> Self {
        Self {
            path,
            function,
            register_count: info.register_count,
            instruction_count: info.instruction_count,
            parameters: info
                .parameters
                .into_iter()
                .map(|parameter| ParameterOutput {
                    name: parameter.name,
                    register: parameter.register,
                    ty: slot_type_name(parameter.ty),
                })
                .collect(),
            regions: info
                .regions
                .into_iter()
                .map(|region| RegionOutput {
                    id: region.id.0,
                    header: region.header,
                    backedge: region.backedge,
                    exits: region.exits,
                    live_slots: region
                        .live_slots
                        .into_iter()
                        .map(|slot| LiveSlotOutput {
                            register: slot.register,
                            ty: slot_type_name(slot.ty),
                        })
                        .collect(),
                })
                .collect(),
        }
    }
}

#[derive(Serialize)]
struct ParameterOutput {
    name: String,
    register: u16,
    #[serde(rename = "type")]
    ty: &'static str,
}

#[derive(Serialize)]
struct RegionOutput {
    id: usize,
    header: usize,
    backedge: usize,
    exits: Vec<usize>,
    live_slots: Vec<LiveSlotOutput>,
}

#[derive(Serialize)]
struct LiveSlotOutput {
    register: u16,
    #[serde(rename = "type")]
    ty: &'static str,
}

pub(super) fn print_inspection(function: &str, info: &ExecutableInfo) {
    println!("Function: {function}");
    println!("Parameters: {}", info.parameters.len());
    for parameter in &info.parameters {
        println!(
            "  {}: r{} {}",
            parameter.name,
            parameter.register,
            slot_type_name(parameter.ty)
        );
    }
    println!("Registers: {}", info.register_count);
    println!("Instructions: {}", info.instruction_count);
    println!("Regions: {}", info.regions.len());

    for region in &info.regions {
        println!();
        println!("Region {}", region.id.0);
        println!("  Header: {}", region.header);
        println!("  Backedge: {}", region.backedge);
        let exits = region
            .exits
            .iter()
            .map(usize::to_string)
            .collect::<Vec<_>>()
            .join(", ");
        println!(
            "  Exits: {}",
            if exits.is_empty() { "none" } else { &exits }
        );
        println!("  Live slots:");
        if region.live_slots.is_empty() {
            println!("    none");
        } else {
            for slot in &region.live_slots {
                println!("    r{}: {}", slot.register, slot_type_name(slot.ty));
            }
        }
    }
}
