# Numeric semantics evidence

## Red-to-green toggle

- Scenario: exact equality for `BigInt(2^53 + 1)` versus `f64(2^53)`.
- Red invocation: `cargo test --test numeric_semantics mixed_bigint_and_float_equality_is_exact_beyond_f64_integer_precision -- --exact`
- Red observable: exit code `101`; assertion reported `left: Bool(true)`, `right: Bool(false)`.
- Green invocation: `cargo test --test numeric_semantics`
- Green observable: exit code `0`; `11 passed; 0 failed`, including exact SmallInt/BigInt equality and ordering, distinct dict keys, NaN, controlled overflow, scaled division, and cyclic list/dict cases.

## Success criteria

| Scenario | Invocation | Binary observable | Artifact |
| --- | --- | --- | --- |
| Exact SmallInt/BigInt versus f64 equality and ordering at `2^53 + 1` | `cargo test --test numeric_semantics` | `mixed_smallint... ok`, `mixed_bigint...equality... ok`, `mixed_bigint...ordering... ok` | This file and test-run transcript captured in the executor turn |
| Exact mixed numeric dictionary keys | `cargo test --test numeric_semantics` | `numerically_distinct_bigint_and_float_remain_distinct_dict_keys ... ok` | `tests/numeric_semantics.rs` |
| NaN equality and ordering | `cargo test --test numeric_semantics` | `nan_is_not_equal_to_itself ... ok`; `nan_ordering_returns_a_controlled_error ... ok` | `tests/numeric_semantics.rs` |
| Huge BigInt plus float is controlled | `cargo test --test numeric_semantics` | `huge_bigint_plus_float_returns_a_controlled_error ... ok` | `tests/numeric_semantics.rs` |
| 400-digit `N/N` is `1.0`; `N/1` is controlled | `cargo test --test numeric_semantics` | both `huge_bigint_divided...` cases `ok` | `tests/numeric_semantics.rs` |
| Identical stale ObjectRef is heap-validated | `cargo test identical_stale_object_references_are_validated_before_equality --lib` | `1 passed; 0 failed` | `src/wvm/equality.rs` unit regression |
| Distinct cyclic list/dict equality terminates | `cargo test --test numeric_semantics` | both `numeric_semantics_cycles` cases `ok` | `tests/numeric_semantics/cycles.rs` |

## Quality gates

- Full suite: `cargo test` exited `0`; every unit, integration, and doc-test target passed.
- Lint: `cargo clippy --all-targets --all-features -- -D warnings` exited `0`.
- Formatting: `cargo fmt --all -- --check` exited `0`.
- Pure LOC check invocation: `awk '!/^[[:space:]]*$/ && !/^[[:space:]]*\/\//' <file> | wc -l`.
- Pure LOC observables: arithmetic `215`, equality `175`, numeric helper `120`, primary regression file `222`, cycle regression module `68`.
- Environment note: `omo ulw-loop status --json` could not resolve an attempt directory because `node` is unavailable, so evidence is recorded under `.omo/evidence/` as required. The mandated `rtk` wrapper was also unavailable; direct Cargo/Rust commands were used.
