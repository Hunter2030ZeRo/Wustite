# Guest recursion depth evidence

## Success scenarios

| Criterion | Scenario | Invocation | Binary observable | Artifact |
|---|---|---|---|---|
| Unbounded direct guest recursion returns a controlled typed execution error | `direct_guest_recursion_returns_an_execution_error_at_the_call_depth_limit` | `cargo test --test call_depth -- --nocapture` | `RuntimeError::Execution` contains `guest call depth limit`; test process exits 0 instead of SIGABRT | `green-call-depth-and-runtime.log` |
| Depth state unwinds after an error and finite nested sibling runtimes remain usable | `finite_nested_guest_calls_succeed_after_a_depth_limit_error` | `cargo test --test call_depth -- --nocapture` | The same `Runtime` returns `RuntimeValue::SmallInt(42)` through root → middle → leaf after the recursion error | `green-call-depth-and-runtime.log` |
| Same-ID recursive activations retain nested runtime/profile updates | `same_function_nested_activations_preserve_runtime_profile_updates` | `cargo test --test call_depth same_function_nested_activations_preserve_runtime_profile_updates -- --exact` | Persistent profile count is exactly 256 entries rather than the stale outer activation's 2 | `green-runtime-persistence-exact.log` |
| Existing callable and persistent runtime/JIT behavior is preserved | Python rich-values and runtime API integration suites | `cargo test --test python_rich_values --test runtime_api --test call_depth` | 24 tests pass, including closureless callbacks and repeated JIT reuse | `integration-tests.log` |
| Owned code compiles and is formatted | All targets plus owned Rust files | `cargo check --all-targets`; owned `rustfmt --check`; owned `git diff --check` | All three exit 0 | `cargo-check.log`, `rustfmt-owned.log`, `diff-check.log` |

## Regression proof

- Before the guard, direct recursion aborted the Rust test process with host stack overflow (`red-direct-recursion.log`).
- Temporarily bypassing the same-ID runtime handoff made the persistence regression fail with only the outer activation's state (`red-runtime-persistence.log`). The bypass was removed before final validation.

## Repository-wide gate blockers outside assignment

- `cargo test --all-targets` reaches and passes all three call-depth tests, then fails in concurrently edited `tests/object_invariants.rs::bigint_sequence_indices_support_negative_indexing_and_range_errors` with `sequence index must be a SmallInt` (`full-test.log`).
- `cargo clippy --all-targets --all-features -- -D warnings` is blocked only by `clippy::items-after-test-module` in concurrently edited `src/wvm/equality.rs` (`clippy.log`).
