use super::super::heap::{GcConfig, GcHeap};
use super::super::objects::{AttrValue, DenseObject, MethodTable};
use super::super::shapes::{ShapeError, ShapeTable};
use super::super::symbols::SymbolTable;
use super::super::value_word::{ScalarValue, ValueWord};
use std::thread;

#[test]
fn immutable_shapes_reuse_transitions_and_keep_dense_slots_class_qualified() {
    let mut symbols = SymbolTable::new();
    let name = symbols.intern("field").expect("field symbol");
    let owned_name = String::from("temporary");
    let temporary = symbols.intern(&owned_name).expect("temporary symbol");
    drop(owned_name);
    assert_eq!(symbols.resolve(temporary), Ok("temporary"));

    let mut shapes = ShapeTable::new(symbols.namespace());
    let first_class = shapes.new_class();
    let second_class = shapes.new_class();
    let first_root = shapes.root_shape(first_class).expect("first root");
    let second_root = shapes.root_shape(second_class).expect("second root");
    let first_child = shapes.transition(first_root, name).expect("transition");
    assert_eq!(shapes.transition(first_root, name), Ok(first_child));
    assert_ne!(
        first_child,
        shapes.transition(second_root, name).expect("other class")
    );
    assert_eq!(
        shapes.transition_for_class(second_class, first_child, name),
        Err(ShapeError::WrongClass)
    );
    assert_eq!(shapes.slot(first_child, name), Ok(0));
    assert_eq!(
        shapes
            .transition(first_child, temporary)
            .expect("second slot"),
        shapes
            .transition(first_child, temporary)
            .expect("shared second slot")
    );

    let foreign_symbols = SymbolTable::new();
    let foreign = foreign_symbols.namespace();
    assert_eq!(
        ShapeTable::new(foreign).shape(first_child),
        Err(ShapeError::WrongRuntime)
    );
}

#[test]
fn dense_existing_write_preserves_shape_and_method_paths_have_distinct_allocation_semantics() {
    let heap = GcHeap::new(GcConfig::default());
    let mut symbols = SymbolTable::new();
    let field = symbols.intern("field").expect("field symbol");
    let method = symbols.intern("method").expect("method symbol");
    let mut shapes = ShapeTable::new(symbols.namespace());
    let class = shapes.new_class();
    let root = shapes.root_shape(class).expect("root shape");
    let mut object = DenseObject::new(&heap, root).expect("object allocation");
    let one = ValueWord::encode_scalar(ScalarValue::Integer(1), &heap).expect("word");
    let two = ValueWord::encode_scalar(ScalarValue::Integer(2), &heap).expect("word");
    object
        .set_field(&heap, &mut shapes, field, one)
        .expect("new field");
    let shaped = object.shape();
    object
        .set_field(&heap, &mut shapes, field, two)
        .expect("existing field");
    assert_eq!(object.shape(), shaped);
    assert_eq!(object.get_field(&shapes, field), Ok(two));

    let mut methods = MethodTable::new(symbols.namespace());
    let target = methods.define(class, method).expect("method target");
    let direct = methods
        .resolve_direct(&shapes, &object, method)
        .expect("direct method");
    assert_eq!(direct.target(), target);
    assert_eq!(direct.receiver(), object.handle());
    assert_eq!(methods.bound_materializations(), 0);
    let first = methods
        .resolve_attr(&shapes, &object, method)
        .expect("escaped method");
    let second = methods
        .resolve_attr(&shapes, &object, method)
        .expect("cached escaped method");
    assert!(matches!(first, AttrValue::BoundMethod(_)));
    assert_eq!(first, second);
    assert_eq!(methods.bound_materializations(), 1);
    let replacement = methods
        .invalidate(class, method)
        .expect("callee invalidation");
    assert_ne!(replacement, target);
    assert_eq!(
        methods
            .resolve_direct(&shapes, &object, method)
            .expect("replacement direct")
            .target(),
        replacement
    );
    assert_ne!(
        methods
            .resolve_attr(&shapes, &object, method)
            .expect("replacement escaped"),
        first
    );
    assert_eq!(methods.bound_materializations(), 2);

    let before = shapes.key(object.shape()).expect("shape key");
    shapes.invalidate_class(class).expect("invalidate class");
    assert!(!shapes.key_is_current(before));
}

#[test]
fn runtime_symbol_namespaces_remain_distinct_across_mutator_threads() {
    let namespaces: Vec<_> = (0..8)
        .map(|_| thread::spawn(|| SymbolTable::new().namespace()))
        .map(|thread| thread.join().expect("symbol mutator should finish"))
        .collect();
    for (index, namespace) in namespaces.iter().enumerate() {
        assert!(!namespaces[..index].contains(namespace));
    }
}
