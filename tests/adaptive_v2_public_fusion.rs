use std::process::{Command, Output};

use serde_json::Value;

fn run(path: &str) -> Output {
    Command::new(env!("CARGO_BIN_EXE_wustite"))
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .args([
            "run",
            path,
            "--runtime-core",
            "adaptive-v2",
            "--backend",
            "cranelift",
            "--repeat",
            "3",
            "--json",
        ])
        .output()
        .expect("adaptive-v2 CLI")
}

#[test]
fn live_public_object_traces_use_fused_native_entries() {
    // Given: the three public object-heavy programs executed by the real CLI.
    let cases = [
        ("benchmarks/adaptive_shape_objects.py", 4_096),
        ("benchmarks/adaptive_list_objects.py", 2_016),
        ("benchmarks/adaptive_call_objects.py", 24_512),
    ];

    // When: each retained adaptive-v2 runtime observes three complete executions.
    for (path, expected) in cases {
        let output = run(path);
        assert!(
            output.status.success(),
            "{path}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        let document: Value = serde_json::from_slice(&output.stdout).expect("CLI JSON");
        let adaptive = &document["adaptive_v2"];

        // Then: exact semantics come from accepted machine traces without helper dispatch.
        assert_eq!(
            document["runs"]
                .as_array()
                .expect("runs")
                .iter()
                .map(|run| run["value"]["value"].as_i64())
                .collect::<Vec<_>>(),
            vec![Some(expected); 3],
            "{path}"
        );
        assert!(
            adaptive["machine_entries"]
                .as_u64()
                .is_some_and(|value| value > 0),
            "{path}: {adaptive}"
        );
        assert_eq!(adaptive["helper_calls"], 0, "{path}: {adaptive}");
        assert_eq!(adaptive["generic_dispatch_calls"], 0, "{path}: {adaptive}");
        assert_eq!(adaptive["deopts"], 0, "{path}: {adaptive}");
    }
}
