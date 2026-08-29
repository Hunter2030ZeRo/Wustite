use std::process::{Command, Output};

use serde_json::Value;

const LEGACY_JIT_KEYS: &[&str] = &[
    "compilation_attempts",
    "compiled_regions",
    "tier2_compilation_attempts",
    "tier2_compiled_regions",
    "disabled_regions",
    "native_executions",
    "tier2_native_executions",
    "last_resume_pc",
    "last_exit_kind",
    "helper_calls",
    "guest_calls",
    "call_sites",
    "runtime_ops",
    "exits",
    "calls",
    "native_calls",
    "failures",
];

const LEGACY_HELPER_CALL_KEYS: &[&str] =
    &["call", "get_item", "set_item", "length", "object_access"];
const LEGACY_GUEST_CALL_KEYS: &[&str] = &["direct_native", "interpreter_fallback"];
const LEGACY_CALL_SITE_KEYS: &[&str] = &[
    "leaf_plans",
    "prepared_leaf_hit",
    "compiled_leaf_hit",
    "inlined_leaf",
    "prepared_call_hit",
    "call_guard_miss",
    "megamorphic_fallback",
];
const LEGACY_RUNTIME_OP_KEYS: &[&str] = &[
    "load_constant",
    "binary",
    "compare",
    "unary",
    "boolean",
    "build_tuple",
    "build_list",
    "build_dict",
    "other",
];
const LEGACY_EXIT_KEYS: &[&str] = &["region_exit", "replay_instruction", "deopt"];

fn run_cli(arguments: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_wustite"))
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .args(arguments)
        .output()
        .unwrap()
}

fn assert_object_keys(value: &Value, keys: &[&str]) {
    let object = value.as_object().unwrap();
    for key in keys {
        assert!(object.contains_key(*key), "missing JSON key {key}");
    }
}

fn assert_u64_fields(value: &Value, keys: &[&str]) {
    assert_object_keys(value, keys);
    for key in keys {
        assert!(value[*key].is_u64(), "JSON key {key} is not u64");
    }
}

fn assert_u64_map(value: &Value) {
    let object = value.as_object().unwrap();
    for count in object.values() {
        assert!(count.is_u64());
    }
}

#[test]
fn json_run_preserves_legacy_jit_report_shape() {
    // Given: a hot scalar loop run through the public JSON command surface.
    let output = run_cli(&[
        "run",
        "tests/fixtures/jit_failure.py",
        "--backend",
        "cranelift",
        "--hot-threshold",
        "1",
        "--json",
    ]);

    // When: the command emits its machine-consumed run document.
    let document: Value = serde_json::from_slice(&output.stdout).unwrap();
    let run = &document["runs"][0];
    let jit = &run["jit"];

    // Then: stdout carries the complete existing JSON shape and stderr stays clean.
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(output.stderr.is_empty());
    assert_object_keys(
        &document,
        [
            "path",
            "function",
            "execution_mode",
            "compiler_backend",
            "hot_threshold",
            "profile_cache",
            "runs",
        ]
        .as_slice(),
    );
    assert_object_keys(run, ["index", "value", "jit"].as_slice());
    assert_object_keys(jit, LEGACY_JIT_KEYS);
    assert_eq!(document["execution_mode"], "adaptive_jit");
    assert_eq!(document["compiler_backend"], "cranelift");
    assert_eq!(run["value"]["type"], "small_int");
    assert_eq!(run["value"]["value"], 40320);
    assert!(run["index"].is_u64());
    assert!(document["hot_threshold"].is_u64());
    assert_u64_fields(
        jit,
        [
            "compilation_attempts",
            "compiled_regions",
            "tier2_compilation_attempts",
            "tier2_compiled_regions",
            "disabled_regions",
            "native_executions",
            "tier2_native_executions",
        ]
        .as_slice(),
    );
    assert!(jit["last_resume_pc"].is_null() || jit["last_resume_pc"].is_u64());
    assert!(jit["last_exit_kind"].is_null() || jit["last_exit_kind"].is_string());
    assert_u64_fields(&jit["helper_calls"], LEGACY_HELPER_CALL_KEYS);
    assert_u64_fields(&jit["guest_calls"], LEGACY_GUEST_CALL_KEYS);
    assert_u64_fields(&jit["call_sites"], LEGACY_CALL_SITE_KEYS);
    assert_u64_fields(&jit["runtime_ops"], LEGACY_RUNTIME_OP_KEYS);
    assert_u64_fields(&jit["exits"], LEGACY_EXIT_KEYS);
    assert_u64_map(&jit["calls"]);
    assert_u64_map(&jit["native_calls"]);
    assert!(jit["failures"].is_array());
}

#[test]
fn debug_diagnostics_stay_on_stderr() {
    // Given: a valid debug invocation for a hot scalar loop.
    let debug = run_cli(&[
        "run",
        "tests/fixtures/jit_failure.py",
        "--backend",
        "cranelift",
        "--hot-threshold",
        "1",
        "--debug-jit",
    ]);

    // When: the caller requests JIT diagnostics.
    let debug_stdout = String::from_utf8(debug.stdout).unwrap();
    let debug_stderr = String::from_utf8(debug.stderr).unwrap();

    // Then: program output and non-JSON diagnostics keep their distinct streams.
    assert!(debug.status.success(), "{debug_stderr}");
    assert_eq!(debug_stdout.trim(), "40320");
    assert!(!debug_stderr.is_empty());
    assert!(serde_json::from_str::<Value>(&debug_stderr).is_err());
}

#[test]
fn malformed_backend_fails_on_stderr() {
    // Given: a malformed enum value at the CLI boundary.
    let malformed = run_cli(&["run", "examples/fp.py", "--backend", "invalid"]);

    // When: clap parses the command line.
    let malformed_stderr = String::from_utf8(malformed.stderr).unwrap();

    // Then: the invalid input fails without contaminating stdout.
    assert!(!malformed.status.success());
    assert!(malformed.stdout.is_empty());
    assert!(!malformed_stderr.is_empty());
}

#[test]
fn debug_jit_format_option_is_rejected() {
    // Given: the unsupported JSON-diagnostic option on the legacy debug surface.
    let output = run_cli(&[
        "run",
        "tests/fixtures/jit_failure.py",
        "--debug-jit",
        "--debug-jit-format",
        "json",
    ]);

    // When: clap parses the command line.
    let stderr = String::from_utf8(output.stderr).unwrap();

    // Then: no program output or JSON diagnostic document is emitted.
    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
    assert!(!stderr.is_empty());
}
