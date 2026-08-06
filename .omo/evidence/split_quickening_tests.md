# WVM quickening test-module extraction evidence

## Scenario: runtime identity and invalid executable test relocation

- Invocation: `cargo test --lib wvm::tests`
- Binary observable: `wvm::tests::quick_code_runtime_identity` and `wvm::tests::invalid_executable_builds_no_quick_runtime` both passed.
- Result: 2 passed, 0 failed, 0 ignored.

## Scenario: quickening construction and execution test relocation

- Invocation: `cargo test --lib wvm::quickening::tests`
- Binary observable: all four moved construction/execution tests passed, including the guard-miss side-effect invariant.
- Result: 4 passed, 0 failed, 0 ignored.

## Scenario: formatting and diff integrity

- Invocation: `cargo fmt --all`, then `cargo fmt --all -- --check && git diff --check`
- Binary observable: both checks exited successfully with no output.

## Scenario: pure LOC ceiling

- Invocation: `awk '!/^[[:space:]]*$/ && !/^[[:space:]]*(//|#|--)/' <file> | wc -l`
- Binary observable: every touched Rust file is at or below 250 pure LOC.

| File | Pure LOC |
| --- | ---: |
| `src/wvm.rs` | 210 |
| `src/wvm/quickening.rs` | 116 |
| `src/wvm/tests/runtime_identity.rs` | 48 |
| `src/wvm/quickening/tests/mod.rs` | 2 |
| `src/wvm/quickening/tests/construction.rs` | 134 |
| `src/wvm/quickening/tests/execution.rs` | 126 |
