#![cfg(feature = "inkwell")]

use std::process::Command;

#[test]
fn bench_uses_selected_llvm_backend() {
    // Given: the benchmark command with LLVM selected as its JIT comparison backend.
    let output = Command::new(env!("CARGO_BIN_EXE_wustite"))
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .args([
            "bench",
            "examples/sum.py",
            "--backend",
            "llvm",
            "--warmup",
            "0",
            "--iterations",
            "1",
            "--hot-threshold",
            "10",
        ])
        .output()
        .unwrap();

    // When: the real CLI completes its cold JIT benchmark.
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();

    // Then: the report identifies LLVM and proves direct Tier-2 compilation occurred.
    assert!(stdout.contains("Compiler backend: llvm"));
    assert!(stdout.contains("Tier-2 compiled regions: 1"));
}
