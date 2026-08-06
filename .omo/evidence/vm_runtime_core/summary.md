# WVM runtime core evidence

Date: 2026-08-06

## Rich interpreter and heap behavior

- Scenario: execute float, Boolean, comparison, string, tuple, list, dict, indexing,
  mutation, length, and SmallInt-to-BigInt promotion through the public VM surface.
- Invocation: `cargo test --test rich_values`
- Binary observable: 7 passed; 0 failed.
- Corroborating frontend scenario: `cargo test --test python_rich_values`
- Binary observable: 8 passed; 0 failed, including closureless function calls and
  BigInt results beyond i64.

## Checked JIT overflow replay

- Scenario: native checked SmallInt addition exits before updating its destination,
  then the interpreter replays the instruction and promotes to `Object::BigInt`.
- Invocations:
  - `cargo test --test jit_region compiled_overflow_exits_before_updating_destination`
  - `cargo test --test jit_region vm_replays_synthetic_overflow_exit_in_interpreter`
- Binary observable: each selected test passed; the final full `cargo test` run
  reports all 8 `jit_region` tests passed.

## Invocation-local region admission

- Scenario: compile and cache a SmallInt-specialized region, invoke it with an Object
  live value, verify interpreter fallback without failure/disable, then invoke with a
  SmallInt again and reuse the same cached native region.
- Invocation:
  `cargo test --test jit_region vm_suppresses_cached_region_for_object_entry_without_disabling_it`
- Binary observable: 1 passed; 0 failed. The scenario asserts zero disabled regions,
  zero failures, zero native executions for the Object invocation, and one cached
  native execution for the following SmallInt invocation.

## Public object/ABI integration

- Scenario: allocate/read/kind-check objects and execute typed host arguments through
  the VM-owned heap.
- Invocations:
  - `cargo test --test object_heap`
  - `cargo test --test runtime_api`
- Binary observable: 4/4 and 11/11 tests passed.

## Quality gates

- Invocation: `rustfmt --edition 2024 --check src/wvm.rs src/wvm/arithmetic.rs
  src/wvm/callables.rs src/wvm/equality.rs src/wvm/interpreter.rs
  src/wvm/jit_runtime.rs src/wvm/objects.rs src/wvm/registers.rs
  src/jit/compiled_region.rs`
- Binary observable: exit 0.
- Invocation: `cargo clippy --all-targets --all-features -- -D warnings`
- Binary observable: exit 0.
- Invocation: `cargo test`
- Binary observable: exit 0; all unit, integration, and doc-test targets passed.
- Invocation: Miri Level 3 and Tree Borrows runs of `cargo +nightly miri test
  --test rich_values` with strict provenance, symbolic alignment, preemption, full
  backtraces, and isolation disabled.
- Binary observable: both runs passed 7/7. A direct strict-provenance Miri run of
  native Cranelift execution stops inside `cranelift-jit`'s integer-to-pointer cast
  before reaching Wustite native code; the real native path is covered by the passing
  `jit_region` suite.

## Size and production-safety audit

- Scenario: measure every owned Rust file using nonblank, non-comment pure LOC.
- Invocation: `awk '!/^[[:space:]]*$/ && !/^[[:space:]]*\/\//' <file> | wc -l`
- Binary observable: wvm.rs 178; arithmetic.rs 247; callables.rs 62; equality.rs 100;
  interpreter.rs 212; jit_runtime.rs 152; objects.rs 169; registers.rs 21;
  compiled_region.rs 162. Every file is at or below 250 pure LOC. `arithmetic.rs` is
  in the 200-250 warning band and should be split before its next material expansion.
- Invocation: `rg -n 'unwrap\(|expect\(|panic!|unreachable!'` over all owned files.
- Binary observable: no matches.
