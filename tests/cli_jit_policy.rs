use std::process::{Command, Output};

use serde_json::Value;

fn run_cli(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_wustite"))
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .args(args)
        .output()
        .unwrap()
}

fn stderr(output: &Output) -> String {
    String::from_utf8(output.stderr.clone()).unwrap()
}

#[test]
fn structure_map_default_jit_policy() {
    // Given: a threshold that runtime profiling cannot reach.
    let output = run_cli(&[
        "run",
        "examples/sum.py",
        "--hot-threshold",
        &u64::MAX.to_string(),
        "--json",
    ]);

    // When: the program runs without an explicit JIT policy.
    assert!(output.status.success(), "{}", stderr(&output));
    let document: Value = serde_json::from_slice(&output.stdout).unwrap();

    // Then: the static map cannot bypass runtime profile readiness.
    assert_eq!(document["runs"][0]["jit"]["compilation_attempts"], 0);
    assert_eq!(document["runs"][0]["jit"]["native_executions"], 0);
}

#[test]
fn profile_jit_policy_waits_hot_threshold() {
    // Given: profile-guided planning and a threshold that the loop cannot reach.
    let output = run_cli(&[
        "run",
        "examples/sum.py",
        "--jit-policy",
        "profile",
        "--hot-threshold",
        &u64::MAX.to_string(),
        "--json",
    ]);

    // When: the program completes one interpreted invocation.
    assert!(output.status.success(), "{}", stderr(&output));
    let document: Value = serde_json::from_slice(&output.stdout).unwrap();

    // Then: no native region is compiled or executed.
    assert_eq!(document["runs"][0]["jit"]["compilation_attempts"], 0);
    assert_eq!(document["runs"][0]["jit"]["native_executions"], 0);
}

#[test]
fn structure_map_jit_policy_selected_explicitly() {
    // Given: explicit StructureMap planning and an unreachable profile threshold.
    let output = run_cli(&[
        "run",
        "examples/sum.py",
        "--jit-policy",
        "structure-map",
        "--hot-threshold",
        &u64::MAX.to_string(),
        "--json",
    ]);

    // When: the program executes the statically mapped loop.
    assert!(output.status.success(), "{}", stderr(&output));
    let document: Value = serde_json::from_slice(&output.stdout).unwrap();

    // Then: explicit selection still cannot bypass runtime profile readiness.
    assert_eq!(document["runs"][0]["jit"]["compilation_attempts"], 0);
    assert_eq!(document["runs"][0]["jit"]["native_executions"], 0);
}

#[test]
fn structure_map_compiles_after_hot_validation() {
    let output = run_cli(&[
        "run",
        "examples/sum.py",
        "--jit-policy",
        "structure-map",
        "--hot-threshold",
        "8",
        "--json",
    ]);

    assert!(output.status.success(), "{}", stderr(&output));
    let document: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(document["runs"][0]["jit"]["compilation_attempts"], 1);
    assert_eq!(document["runs"][0]["jit"]["native_executions"], 1);
}
