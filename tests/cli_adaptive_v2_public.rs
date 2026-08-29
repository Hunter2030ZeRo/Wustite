use std::process::{Command, Output};

use serde_json::Value;

fn run(arguments: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_wustite"))
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .args(arguments)
        .output()
        .unwrap()
}

#[test]
fn explicit_adaptive_core_emits_versioned_json_after_real_machine_entry() {
    // Given: an explicit adaptive-v2 execution with enough live entry samples.
    let output = run(&[
        "run",
        "tests/fixtures/adaptive_add.py",
        "--runtime-core",
        "adaptive-v2",
        "--arg",
        "20",
        "--arg",
        "22",
        "--repeat",
        "100",
        "--json",
    ]);

    // When: stdout is decoded as the machine-consumed document.
    let document: Value = serde_json::from_slice(&output.stdout).unwrap();
    let adaptive = &document["adaptive_v2"];

    // Then: stdout stays JSON, stderr stays clean, and native counters are honest.
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(output.stderr.is_empty());
    assert_eq!(adaptive["schema_version"], 1);
    assert_eq!(adaptive["runtime_core"], "adaptive_v2");
    assert!(adaptive["machine_entries"].as_u64().unwrap() > 0);
    assert_eq!(adaptive["generic_dispatch_calls"], 0);
    assert_eq!(document["runs"][99]["value"]["value"], 42);
}

#[test]
fn legacy_default_json_and_inert_command_policy_remain_unchanged() {
    // Given: legacy default execution and an adaptive-only option on inspect.
    let legacy = run(&[
        "run",
        "tests/fixtures/adaptive_add.py",
        "--arg",
        "20",
        "--arg",
        "22",
        "--json",
    ]);
    let inert = run(&[
        "inspect",
        "tests/fixtures/adaptive_add.py",
        "--runtime-core",
        "adaptive-v2",
    ]);

    // When: callers inspect the two command boundaries.
    let legacy_json: Value = serde_json::from_slice(&legacy.stdout).unwrap();

    // Then: legacy JSON gains no adaptive key and inert commands reject the option.
    assert!(legacy.status.success());
    assert!(legacy_json.get("adaptive_v2").is_none());
    assert!(!inert.status.success());
    assert!(inert.stdout.is_empty());
    assert!(!inert.stderr.is_empty());
}

#[test]
fn adaptive_human_diagnostics_are_stderr_only() {
    // Given: adaptive execution with human diagnostics enabled.
    let output = run(&[
        "run",
        "tests/fixtures/adaptive_add.py",
        "--runtime-core",
        "adaptive-v2",
        "--arg",
        "20",
        "--arg",
        "22",
        "--repeat",
        "100",
        "--debug-jit",
    ]);

    // When/Then: results remain stdout while versioned adaptive state is stderr.
    assert!(output.status.success());
    assert!(
        String::from_utf8(output.stdout)
            .unwrap()
            .contains("run 100: 42")
    );
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.contains("adaptive-v2 schema=1"), "{stderr}");
    assert!(stderr.contains("machine_entries="), "{stderr}");
}

#[cfg(feature = "inkwell")]
#[test]
fn direct_llvm_is_rejected_before_adaptive_execution() {
    // Given/When: adaptive-v2 is requested with an unsupported direct LLVM entry tier.
    let output = run(&[
        "run",
        "tests/fixtures/adaptive_add.py",
        "--runtime-core",
        "adaptive-v2",
        "--backend",
        "llvm",
        "--arg",
        "20",
        "--arg",
        "22",
    ]);

    // Then: no result is printed and stderr names the required tier ordering.
    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
    assert!(
        String::from_utf8(output.stderr)
            .unwrap()
            .contains("requires Cranelift tier-1 before LLVM promotion")
    );
}

#[cfg(feature = "inkwell")]
#[test]
fn tiered_cli_reports_same_snapshot_llvm_promotion_after_real_execution() {
    // Given: the public CLI selects adaptive tiering for a retained scalar entry.
    let output = run(&[
        "run",
        "tests/fixtures/adaptive_add.py",
        "--runtime-core",
        "adaptive-v2",
        "--backend",
        "tiered",
        "--arg",
        "20",
        "--arg",
        "22",
        "--repeat",
        "107",
        "--json",
    ]);

    // When: stdout is decoded after the tier-2 threshold is crossed.
    let document: Value = serde_json::from_slice(&output.stdout).unwrap();
    let adaptive = &document["adaptive_v2"];

    // Then: the actual result and exact same-snapshot promotion are externally visible.
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(document["runs"][106]["value"]["value"], 42);
    assert_eq!(adaptive["compile_tier"], "llvm-o3");
    assert_eq!(adaptive["tier1_snapshot_id"], adaptive["tier2_snapshot_id"]);
    assert_eq!(adaptive["generic_dispatch_calls"], 0);
}

#[test]
fn cranelift_cli_executes_both_guarded_loops_without_bridge_or_dispatch() {
    // Given: one public program warms a boolean callee before exercising its opposite case.
    let output = run(&[
        "run",
        "tests/fixtures/adaptive_guarded_bool_cli.py",
        "--runtime-core",
        "adaptive-v2",
        "--backend",
        "cranelift",
        "--json",
    ]);

    // When: the machine-consumed adaptive report is decoded.
    let document: Value = serde_json::from_slice(&output.stdout).unwrap();
    let adaptive = &document["adaptive_v2"];

    // Then: guarded preheader entry runs each verified loop snapshot directly. The opposite
    // boolean case therefore needs neither a child bridge nor interpreter replay.
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(document["runs"][0]["value"]["value"], false);
    assert_eq!(adaptive["bridges"], 0);
    assert_eq!(adaptive["guard_failures"], serde_json::json!({}));
    assert_eq!(adaptive["machine_entries"], 2);
    assert_eq!(adaptive["native_executions"], 2);
    assert_eq!(adaptive["guest_calls"], 0);
    assert_eq!(adaptive["helper_calls"], 0);
    assert_eq!(adaptive["deopts"], 0);
    assert_eq!(adaptive["generic_dispatch_calls"], 0);
    assert_eq!(
        adaptive["selected_snapshot_id"],
        adaptive["tier1_snapshot_id"]
    );
}
