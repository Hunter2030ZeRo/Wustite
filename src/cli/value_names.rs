use wustite::object::ObjectKind;
use wustite::structure_map::SlotType;

pub(super) const fn object_kind_name(kind: ObjectKind) -> &'static str {
    match kind {
        ObjectKind::String => "string",
        ObjectKind::Tuple => "tuple",
        ObjectKind::BigInt => "big_int",
        ObjectKind::List => "list",
        ObjectKind::Dict => "dict",
        ObjectKind::Function => "function",
    }
}

pub(super) const fn slot_type_name(ty: SlotType) -> &'static str {
    match ty {
        SlotType::SmallInt => "small_int",
        SlotType::Float => "float",
        SlotType::Bool => "bool",
        SlotType::Object(kind) => object_kind_name(kind),
        SlotType::Any => "any",
    }
}
