use crate::bytecode::Function;
use crate::structure_map::StructureMap;

pub struct ExecutableFunction {
    pub bytecode: Function, 
    pub structure_map: StructureMap,
}