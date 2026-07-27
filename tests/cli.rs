use std::process::{Command, Output};

use serde_json::Value;

fn run_cli(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_wustite"))
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .args(args)
        .output()
        .unwrap()
}

fn stdout(output: &Output) -> String {
    String::from_utf8(output.stdout.clone()).unwrap()
}

fn stderr(output: &Output) -> String {
    String::from_utf8(output.stderr.clone()).unwrap()
}

#[test]
fn help_describes_commands_and_options() {
    let root = run_cli(&["--help"]);
    assert!(root.status.success());
    let root_help = stdout(&root);
    assert!(root_help.contains("run"));
    assert!(root_help.contains("inspect"));

    let run = run_cli(&["run", "--help"]);
    assert!(run.status.success());
    let run_help = stdout(&run);
    for option in [
        "--function",
        "--repeat",
        "--interpreter",
        "--hot-threshold",
        "--trace-jit",
        "--json",
    ] {
        assert!(run_help.contains(option));
    }

    let inspect = run_cli(&["inspect", "--help"]);
    assert!(inspect.status.success());
    let inspect_help = stdout(&inspect);
    assert!(inspect_help.contains("--function"));
    assert!(inspect_help.contains("--json"));
}

#[test]
fn basic_run_prints_only_the_result() {
    let output = run_cli(&["run", "examples/sum.py"]);

    assert!(output.status.success());
    assert_eq!(stdout(&output).trim(), "5050");
    assert!(stderr(&output).is_empty());
}

#[test]
fn repeated_run_reuses_compiled_region_and_traces_to_stderr() {
    let output = run_cli(&[
        "run",
        "examples/sum.py",
        "--hot-threshold",
        "10",
        "--repeat",
        "2",
        "--trace-jit",
    ]);

    assert!(output.status.success());
    assert_eq!(stdout(&output), "run 1: 5050\nrun 2: 5050\n");
    let trace = stderr(&output);
    let second = trace
        .lines()
        .find(|line| line.starts_with("run 2:"))
        .unwrap();
    assert!(second.contains("compilation_attempts=0"));
    assert!(second.contains("compiled_regions=0"));
    assert!(second.contains("native_executions=1"));
}

#[test]
fn interpreter_mode_never_tiers_up() {
    let output = run_cli(&["run", "examples/sum.py", "--interpreter", "--trace-jit"]);

    assert!(output.status.success());
    assert_eq!(stdout(&output).trim(), "5050");
    let trace = stderr(&output);
    assert!(trace.contains("compilation_attempts=0"));
    assert!(trace.contains("native_executions=0"));
}

#[test]
fn json_run_is_one_typed_document_with_jit_snapshots() {
    let output = run_cli(&[
        "run",
        "examples/sum.py",
        "--hot-threshold",
        "10",
        "--repeat",
        "2",
        "--json",
    ]);

    assert!(output.status.success());
    assert!(stderr(&output).is_empty());
    let document: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(document["execution_mode"], "adaptive_jit");
    assert_eq!(document["runs"].as_array().unwrap().len(), 2);
    assert_eq!(document["runs"][0]["value"]["type"], "i64");
    assert_eq!(document["runs"][0]["value"]["value"], 5050);
    assert_eq!(document["runs"][1]["jit"]["compilation_attempts"], 0);
    assert_eq!(document["runs"][1]["jit"]["compiled_regions"], 0);
    assert_eq!(document["runs"][1]["jit"]["native_executions"], 1);
    assert_eq!(document["runs"][1]["jit"]["last_exit_kind"], "region_exit");
}

#[test]
fn inspect_human_output_uses_runtime_metadata() {
    let output = run_cli(&["inspect", "examples/sum.py"]);

    assert!(output.status.success());
    assert!(stderr(&output).is_empty());
    let text = stdout(&output);
    for expected in [
        "Function: main",
        "Registers: 11",
        "Instructions: 16",
        "Regions: 1",
        "Region 0",
        "Header: 8",
        "Backedge: 14",
        "Exits: 15",
        "r0: i64",
        "r6: i64",
    ] {
        assert!(text.contains(expected));
    }
}

#[test]
fn inspect_json_contains_the_same_structured_metadata() {
    let output = run_cli(&["inspect", "examples/sum.py", "--json"]);

    assert!(output.status.success());
    assert!(stderr(&output).is_empty());
    let document: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(document["function"], "main");
    assert_eq!(document["register_count"], 11);
    assert_eq!(document["instruction_count"], 16);
    assert_eq!(document["regions"][0]["id"], 0);
    assert_eq!(document["regions"][0]["header"], 8);
    assert_eq!(document["regions"][0]["backedge"], 14);
    assert_eq!(document["regions"][0]["exits"][0], 15);
    assert_eq!(document["regions"][0]["live_slots"][0]["register"], 0);
    assert_eq!(document["regions"][0]["live_slots"][0]["type"], "i64");
}

#[test]
fn operational_and_usage_errors_use_the_expected_streams_and_codes() {
    let missing = run_cli(&["run", "tests/fixtures/does-not-exist.py"]);
    assert_eq!(missing.status.code(), Some(1));
    assert!(stdout(&missing).is_empty());
    let missing_error = stderr(&missing);
    assert!(missing_error.contains("failed to read"));
    assert!(missing_error.contains("does-not-exist.py"));

    let function = run_cli(&["run", "examples/sum.py", "--function", "does_not_exist"]);
    assert_eq!(function.status.code(), Some(1));
    assert!(stdout(&function).is_empty());
    assert!(stderr(&function).contains("frontend error"));

    let unsupported = run_cli(&["run", "tests/fixtures/unsupported.py"]);
    assert_eq!(unsupported.status.code(), Some(1));
    assert!(stdout(&unsupported).is_empty());
    let unsupported_error = stderr(&unsupported);
    assert!(unsupported_error.contains("line 2"));
    assert!(!unsupported_error.to_lowercase().contains("panic"));

    let repeat = run_cli(&["run", "examples/sum.py", "--repeat", "0"]);
    assert_eq!(repeat.status.code(), Some(2));

    let conflict = run_cli(&["run", "examples/sum.py", "--json", "--trace-jit"]);
    assert_eq!(conflict.status.code(), Some(2));
}
