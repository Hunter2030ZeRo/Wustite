# WXIR builder integration evidence

## Owned-file formatting

- Scenario: every touched/new WXIR builder Rust file is rustfmt-clean.
- Invocation: `rustfmt --edition 2021 --check src/wxir/builder.rs src/wxir/builder/control.rs src/wxir/builder/lowering.rs src/wxir/builder/operations.rs src/wxir/builder/setup.rs src/wxir/builder/state.rs`
- Binary observable: exit status 0.
- Artifact: `rustfmt-check.log` and `owned-gates-status.log`.

## Owned diff integrity

- Scenario: the assigned builder diff has no whitespace errors.
- Invocation: `git diff --check -- src/wxir/builder.rs src/wxir/builder`
- Binary observable: exit status 0.
- Artifact: `git-diff-check.log` and `owned-gates-status.log`.

## Refactor size constraint

- Scenario: each builder module remains at or below 250 pure lines.
- Invocation: `awk` pure-line count over `src/wxir/builder.rs` and `src/wxir/builder/*.rs`.
- Binary observable: maximum observed count 199, below the 250-line ceiling.
- Artifact: `pure-loc.log`.

## Required WXIR builder test

- Scenario: compile and run the `wxir_builder` integration test binary.
- Invocation: `cargo test --test wxir_builder`.
- Binary observable: exit status 1 before test execution due to concurrent, out-of-scope compile errors in `src/frontend/python/lower.rs`, `src/jit/compiled_region.rs`, `src/runtime.rs`, and `src/wvm.rs`; no error references an assigned WXIR builder file.
- Artifact: `final-cargo-test.log`.
