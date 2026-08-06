# Static quickening evidence manifest

Infrastructure exception: `rtk` is unavailable and `omo ulw-loop` cannot launch because Node is missing; direct `cargo`, `git`, and `rg` commands were used as authorized by the approved plan. See `infrastructure-static-quickening.txt`.

| Criterion / scenario | Invocation | Binary observable | Artifact |
|---|---|---|---|
| Task 1 red-first overlay | `cargo test --lib quickening` before definitions | exit 101; unresolved `QuickCode` / `QuickInstruction` | `task-1-red.log` |
| Exact Add/Lt overlay | `cargo test --lib wvm::quickening::tests::construction::quick_code_builds_only_exact_add_and_lt -- --exact` | exit 0; 1 passed after final test extraction | `split_quickening_tests.md` |
| Negative eligibility matrix | `cargo test --lib wvm::quickening::tests::construction::quick_code_preserves_unknown_mismatched_and_unsupported_sites -- --exact` | exit 0; 1 passed after final test extraction | `split_quickening_tests.md` |
| Task 2 red-first helper | `cargo test --lib smallint_add` before helper | exit 101; method not found | `task-2-red.log` |
| In-range shared Add | `cargo test --lib wvm::arithmetic::tests::smallint_add_returns_immediate_when_in_range -- --exact` | exit 0; 1 passed | `task-2-smallint-add.log` |
| Both overflow directions | `cargo test --lib wvm::arithmetic::tests::smallint_add_promotes_both_overflow_directions -- --exact` | exit 0; 1 passed | `task-2-smallint-add-error.log` |
| Existing BigInt arithmetic regression | `cargo test --test rich_values smallint_overflow_promotes_to_bigint_for_following_arithmetic -- --exact` | exit 0; 1 passed | `task-2-rich-values-regression.log` |
| Task 3 red-first executor | `cargo test --lib quick_execution` before executor | exit 101; unresolved executor/outcome | `task-3-red.log` |
| Quick Add/Lt and aliases | `cargo test --lib wvm::quickening::tests::execution::quick_execution_handles_exact_smallints_and_aliases -- --exact` | exit 0; 1 passed after final test extraction | `split_quickening_tests.md` |
| Side-effect-free misses and post-overflow miss | `cargo test --lib wvm::quickening::tests::execution::quick_execution_guard_miss_is_side_effect_free -- --exact` | exit 0; 1 passed after final test extraction | `split_quickening_tests.md` |
| Task 4 red-first Arc lifecycle | `cargo test --lib quick_code_runtime` before runtime field/constructor | exit 101; missing field/constructor | `task-4-red.log` |
| Cache/clone/revision/recursion ownership | Unit identity test plus exact `vm_sum` and `call_depth` lifecycle tests | every invocation exit 0; pointer and existing lifecycle assertions pass | `task-4-runtime-lifecycle.log` |
| Invalid executable creates no runtime | Unit invalid-executable test plus exact cached-runtime regression | both invocations exit 0 | `task-4-runtime-lifecycle-error.log` |
| Task 5 red-first dispatch integration | Exact integration test before hook with a temporary red-first sentinel | exit 101; named test fails | `task-5-red.log` |
| Post-JIT exact dispatch and immutable semantics | `cargo test --test static_quickening exact_add_and_lt_execute_without_mutating_semantic_bytecode -- --exact` | exit 0; 1 passed | `task-5-dispatch.log` |
| Native replay, exact promotion, semantic downstream fallback | `cargo test --test static_quickening semantic_jit_replay_promotes_then_falls_back_for_bigint -- --exact` | exit 0; 1 passed; replay PC 3 assertions hold | `task-5-dispatch-error.log` |
| Production LOC ceiling | Final baseline-isolated zero-context diffs with nonblank/non-comment production additions counted | `total_added_pure_loc: 187` (<=250) after exhaustive matching | `final-pure-loc.txt` |
| Plan/protected-file compliance | Required evidence size checks, LOC gate, and `cmp` against all protected baselines | exit 0 | `f1-plan-compliance.log` |
| Formatting/lint/whitespace | `cargo fmt --all -- --check`; `cargo clippy --all-targets --all-features -- -D warnings`; `git diff --check` | exit 0; no warnings/diagnostics | `final-root-verification.md` |
| Full stable + Tree-Borrows QA | `cargo test --all-targets`; both available nightly Miri lanes with Tree Borrows | exit 0; 122 stable tests pass; Miri quickening 4/4 and integration 2/2 with native fixture ignored | `final-root-verification.md` |
| Scope fidelity | Unrelated status comparison, full quickening integration suite, forbidden-surface search | exit 0; unrelated state identical; 3/3 tests pass; typed-op matches only in negative tests | `f4-scope-fidelity.log` |
| Dispatch/helper/recursion source inspection | Ordered dispatch, single checked promotion, Arc-only recursive placeholder, forbidden production surface searches | one production dispatch; one `checked_add`; no forbidden production match | `final-source-inspection.log` |
| Independent code review | Baseline-isolated artifact-backed audit plus fresh Clippy/tests | APPROVED; no findings | `static-quickening-code-review.md` |
| Independent manual QA | Exact/fallback/overflow/JIT replay/clone/recursion scenarios plus full targets | PASS | `final_qa/static_quickening-manual-qa.md` |
| Final gate | Reproduced fmt, Clippy, 122 tests, Miri, protected-file comparisons, and LOC gate | APPROVED; no blockers | `static-quickening-gate-review.md` |

The executor snapshot is retained in `final-scoped-diff.patch` and `final-status.txt`; the canonical post-cleanup results are in `final-root-verification.md`.
