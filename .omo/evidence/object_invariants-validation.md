# Object invariants validation

Captured 2026-08-06 in `/home/entity27th/Wustite`.

| Success criterion | Scenario | Invocation | Binary observable |
| --- | --- | --- | --- |
| Container references are local and live | `containers_reject_foreign_and_stale_nested_references` allocates a list with a foreign reference and a tuple with a stale reused-slot reference. | `cargo test --test object_invariants` | Exit 0; both cases returned their typed `ObjectError` variants. |
| Containers never retain uninitialized values; host dictionaries reject unhashable/duplicate keys | `containers_reject_uninitialized_values_and_unhashable_dictionary_keys`, `runtime_rejects_host_created_dictionaries_before_runtime_normalization`, and `host_dictionary_rejects_equivalent_string_object_keys`. | `cargo test --test object_invariants` | Exit 0; six invariant regressions passed. |
| Host dictionary numeric equivalence is exact | `host_dictionary_handles_exact_mixed_numeric_keys` rejects `1`/`1.0` and accepts `BigInt(9007199254740993)` with `9007199254740992.0`. | `cargo test --test object_invariants` | Exit 0; exact mixed-numeric regression passed. |
| BigInt sequence indices preserve Python indexing rules | `bigint_sequence_indices_support_negative_indexing_and_range_errors` indexes with `BigInt(-1)` and an oversized BigInt. | `cargo test --test object_invariants` | Exit 0; selected `SmallInt(20)` and returned `sequence index out of range`, respectively. |
| WVM dictionary normalization remains operational | Existing `rich_values` and `python_rich_values` dictionary/indexing scenarios execute through the WVM allocation path. | `cargo test --test object_heap --test object_invariants --test rich_values --test python_rich_values` | Exit 0; 27 tests passed. |
| Repository regression gate | Full unit, integration, and doc test set. | `cargo test` | Exit 0; every listed test target passed. |
| Formatting and linting | Owned Rust files and affected test targets. | `rustfmt --edition 2024 --check src/object/heap.rs src/object/heap/invariants.rs src/wvm/objects.rs tests/object_invariants.rs && cargo clippy --test object_invariants --test object_heap --test rich_values --test python_rich_values -- -D warnings` | Exit 0. |

Post-write review: heap storage (193 pure LOC) and host graph validation (149 pure LOC) have separate responsibilities; object operations (186 pure LOC) and regressions (143 pure LOC) remain below the 250-line limit. The production files contain no `unwrap`, `expect`, or numeric `as` casts. Host dictionary numeric comparison is local and exact, so `ObjectHeap` remains independent of `wvm::equality`.
