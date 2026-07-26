use wustite::jit::run_jit_add;

#[test]
fn jit_compiles_and_executes_add() {
    let result = run_jit_add(20, 22).unwrap();

    assert_eq!(result, 42);
}
