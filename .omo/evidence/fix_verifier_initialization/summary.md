# Verifier initialization evidence

## Success criteria

- Scenario: direct `Return`, `BuildList`, branch join, and loop-backedge reads of unwritten registers; parameters initialized at entry; reachable fallthrough; excessive `register_count`.
  - Invocation: `cargo test --test verifier_initialization --no-fail-fast`
  - Binary observable: exit 0; 7 passed, 0 failed.
  - Artifact: `targeted-tests.log`
- Scenario: existing hand-authored sum loop and verifier metadata cases remain valid.
  - Invocation: `cargo test --test vm_sum --no-fail-fast`
  - Binary observable: exit 0; 10 passed, 0 failed.
  - Artifact: `existing-loop-tests.log`
- Scenario: library lint gate for the verifier implementation.
  - Invocation: `cargo clippy --lib -- -D warnings`
  - Binary observable: exit 0.
  - Artifact: `clippy-lib.log`
- Scenario: owned Rust files are formatted.
  - Invocation: `rustfmt --edition 2021 --check src/verifier.rs src/verifier/initialization.rs tests/verifier_initialization.rs`
  - Binary observable: exit 0.
  - Artifact: `rustfmt-owned.log`
- Scenario: settled shared tree integration.
  - Invocation: `cargo test --all-targets --no-fail-fast`
  - Binary observable: exit 0; every listed test target passed, including 7 verifier-initialization tests and 10 VM loop/verifier tests.
  - Artifact: `full-test.log`
- Scenario: strict all-target lint gate.
  - Invocation: `cargo clippy --all-targets --all-features -- -D warnings`
  - Binary observable: exit 0.
  - Artifact: `clippy-all-targets.log`
- Scenario: repository formatting gate.
  - Invocation: `cargo fmt --all -- --check`
  - Binary observable: exit 0.
  - Artifact: `rustfmt-all.log`

## Integrated-suite note

The settled all-target run passed. During concurrent development an existing WXIR fixture intentionally returned an unwritten register and began failing earlier at the strengthened executable verifier; the root integrator initialized that fixture as a parameter before the final successful run.

## Post-write review

- `src/verifier/initialization.rs` owns definite-assignment dataflow; `src/verifier.rs` owns executable verification orchestration.
- Inputs remain typed bytecode and no unsafe code, unchecked numeric narrowing, logging, or new dependencies were introduced.
- Instruction matches are exhaustive, including reads and writes for every current ISA variant.
- The tests fail if the assignment pass or register cap is removed.
- Pure LOC: `src/verifier.rs` 209 (warning band), initialization module 176, test 125. The dataflow responsibility is already split; further growth in `verifier.rs` should extract bounds/operand validation.
