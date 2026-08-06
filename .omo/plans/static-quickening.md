# Static Quickening for Verified Semantic Bytecode

## TL;DR
> Summary:      Add a private, immutable, PC-indexed quick-code overlay to each cached `FunctionRuntime`, derive only the two WXIR-backed exact specializations, and try it after JIT/OSR declines but before the unchanged semantic instruction executes. Guard misses are side-effect-free and fall through; checked SmallInt addition retains BigInt promotion.
> Deliverables: `QuickCode` construction/execution; verified cache-miss lifecycle and recursive sharing; post-JIT interpreter hook; focused red-first unit/integration coverage; full Rust/Miri/static-quality evidence
> Effort:       Medium
> Risk:         High - dispatch ordering, overflow replay, and same-ID recursive runtime swapping can silently change semantics if ownership or fallback is wrong

## Scope
### Must have
- Add private `quick_code` state to `FunctionRuntime`, backed by a shared immutable overlay with exactly one `Option<QuickInstruction>` per semantic bytecode PC.
- Construct the overlay only on a verified `ExecutableId` cache miss. A verifier failure must not create or replace a runtime; clones and A/B/A execution reuse the cached overlay, while a fresh executable revision gets an independent overlay.
- Populate only these exact matches, including `OperationSite.pc == semantic pc`: `BinaryOp::Add` with `Exact(SmallInt), Exact(SmallInt) -> Exact(SmallInt)`, and `CompareOp::Lt` with `Exact(SmallInt), Exact(SmallInt) -> Exact(Bool)`.
- Keep unknown facts, mismatched exact facts, unsupported semantic operators, legacy typed opcodes, and every other instruction as `None` in the overlay.
- Store only quick-op kind and register indices. Do not store `Value`, `ObjectRef`, heap references, cloned semantic `Instruction`, profile/JIT state, or source metadata.
- Make a failed runtime guard a strict no-op: unchanged PC, registers, heap, and destination. The interpreter must then execute the original semantic instruction at the same PC.
- Execute checked quick Add for two `Value::SmallInt` operands. A non-overflow result is `SmallInt`; overflow allocates the same exact `BigInt` as generic arithmetic. A downstream quick Add/Lt that sees that `Object(BigInt)` must guard-miss and fall back to semantic arithmetic/comparison.
- Execute quick Lt as signed `i64` comparison for two `SmallInt` operands and write `Bool`.
- Attempt quick dispatch only after `try_execute_region(...)` returns `false`, before semantic instruction lookup/dispatch. Preserve one-iteration OSR suppression and native `ReplayInstruction` behavior.
- Share the same immutable overlay into the temporary `FunctionRuntime` used for same-ID recursion instead of rebuilding it.
- Keep all newly added non-test Rust code at or below 250 pure lines in total. This plan interprets “pure LOC <=250” as added production code excluding blanks, comment-only lines, and `#[cfg(test)]` bodies; that is stricter than a per-file ceiling.
- Preserve the already-dirty working tree. Record a scoped baseline before Task 1 and isolate only plan-owned deltas in every review/rollback.

### Must NOT have (guardrails, anti-slop, scope boundaries)
- Do not mutate, replace, clone into, or append quick opcodes to `ExecutableFunction.bytecode()`; its public semantic code remains authoritative and immutable.
- Do not add quick variants to public `Instruction`, change public APIs, or expose quick-code counters/introspection solely for tests.
- Do not change verifier acceptance rules, `StructureMap`, WXIR lowering, Cranelift/JIT inputs, region planning, side-exit/source-PC mapping, frontend facts, or legacy opcode semantics.
- Do not specialize Subtract/Multiply/Divide, Eq/NotEq/Le/Gt/Ge, Float, Bool arithmetic, BigInt, strings, collections, or legacy `AddI64`/`LtI64`.
- Do not disable a quick slot after a miss; every execution guards afresh and can use the specialization again when operands match.
- Do not write the destination or advance PC before every guard/check needed for the selected outcome has succeeded.
- Do not introduce dependencies, unsafe code, interior mutability, global overlay caches, persistence across processes, or a second identity key.
- Do not run commits, resets, checkouts, cleans, or broad rollback commands. The caller explicitly forbids commits and the existing user changes must remain intact.

## Verification strategy
> Zero human intervention - all verification is agent-executed.
- Test decision: TDD + Rust built-in unit/integration test harness. Within every task, add the named assertion first, capture the expected red result, implement only that task, then capture green.
- QA policy: every task has agent-executed scenarios
- Evidence: `<attemptDir>/task-<N>-<slug>.<ext>` with `<attemptDir>` fixed to `.omo/evidence/` for this execution. `omo ulw-loop status --json` is unusable because its Node launcher is missing, so do not call it or derive an ulw attempt path.
- Baseline isolation: before Task 1, create `.omo/evidence/baseline/`, save `git status --short` to `.omo/evidence/baseline-status.txt`, and copy the plan-owned files plus protected semantic files (`Cargo.toml`, `Cargo.lock`, `src/bytecode.rs`, `src/executable.rs`, `src/structure_map.rs`, `src/verifier.rs`, `src/verifier/structure.rs`, `src/frontend/python/lower/expression.rs`, `src/wvm/jit_runtime.rs`, `src/wxir/builder/setup.rs`, `src/wxir/builder/lowering.rs`, `src/wxir/builder/operations.rs`, `src/wxir/ir.rs`) under matching paths in `.omo/evidence/baseline/`; create empty placeholders for new files. All later diff, LOC, and rollback checks compare against this snapshot rather than `HEAD`, because the repository already contains intentional user edits.
- Command convention: invoke `cargo`, `git`, and `rg` directly. This is an explicit repository-instruction exception for this plan because `rtk` is not installed/resolvable in the current environment. Do not install `rtk`, Node, nightly, Miri, or any other tool; installation would require out-of-scope environment/network mutation. If a pre-existing nightly/Miri toolchain is absent, record that one Miri lane as infrastructure-blocked while still running every stable direct command.
- Pure-LOC gate: compare `.omo/evidence/baseline/src/` with the final plan-owned `src/` files using a zero-context diff; count only added nonblank, non-comment production lines outside `#[cfg(test)]` blocks and fail when the sum exceeds 250. Store the counted patch and `total_added_pure_loc: <N>` in `.omo/evidence/final-pure-loc.txt`.

## Execution strategy
### Parallel execution waves
> Target 5-8 tasks per wave. <3 per wave (except final) = under-splitting.
> Extract shared dependencies as Wave-1 tasks to maximize parallelism.
>
> This five-step, <=250-production-LOC change has only two safe independent roots; forcing a third root would split implementation from its tests or create concurrent edits to `quickening.rs`. Keep shared-file owners serialized as below.

Wave 1 (no dependencies):
- Task 1: define the PC-indexed overlay and exact-fact constructor
- Task 2: centralize checked SmallInt addition/promotion for reuse

Wave 2 (after Wave 1):
- Task 3: depends [1, 2] - implement guarded quick execution
- Task 4: depends [1] - attach and share quick code through runtime lifecycle

Wave 3 (after Wave 2):
- Task 5: depends [3, 4] - wire post-JIT dispatch and close all public/JIT/lifecycle regressions

Critical path: Task 1 -> Task 3 -> Task 5

### Dependency matrix
| Task | Depends on | Blocks | Can parallelize with |
|------|------------|--------|----------------------|
| 1    | none       | 3, 4   | 2                    |
| 2    | none       | 3      | 1                    |
| 3    | 1, 2       | 5      | 4                    |
| 4    | 1          | 5      | 3                    |
| 5    | 3, 4       | F1-F4  | none                 |

### Team staffing recommendation
```yaml
total_atomic_steps: 5
file_independent_steps: 2
cross_file_dependent_steps: 3
per_step_assignment:
  - step_id: 1
    assigned_to: legacy-executor
    blockedBy: []
    rationale: Defines the invariant-heavy private representation and must keep its eligibility predicate aligned with WXIR.
  - step_id: 2
    assigned_to: legacy-executor
    blockedBy: []
    rationale: Small, localized arithmetic helper extraction following the existing checked-add pattern.
  - step_id: 3
    assigned_to: legacy-executor
    blockedBy: [1, 2]
    rationale: Guard/fallback state safety and BigInt behavior require careful private-unit coverage.
  - step_id: 4
    assigned_to: legacy-executor
    blockedBy: [1]
    rationale: Crosses runtime cache identity and the delicate same-function recursive swap/restore path.
  - step_id: 5
    assigned_to: legacy-executor
    blockedBy: [3, 4]
    rationale: Integrates interpreter, JIT replay, immutable semantic bytecode, and lifecycle regressions end to end.
dispatch_path_recommendation: legacy
dispatch_path_rationale: Only two tasks are file-independent, below the required threshold of three for team dispatch. Use one sequential legacy executor in dependency-safe order 1 -> 2 -> 3 -> 4 -> 5; Tasks 1 and 2 remain independently startable, but must not be staffed as a team wave.
```

## Todos
> Implementation + Test = ONE task. Never separate.
> Every task MUST have: References + Acceptance Criteria + QA Scenarios + Commit.

- [ ] 1. Define the private PC-indexed overlay and exact specialization constructor

  What to do: In a new `src/wvm/quickening.rs`, first add private unit tests for overlay length/indexing and the full eligibility matrix, capture red, then implement `QuickCode` and a compact `QuickInstruction` enum/struct. Register the private module in `src/wvm.rs`. Build a boxed/slice-like overlay whose length exactly equals `bytecode().code.len()`. At each PC, inspect the semantic instruction and its referenced `OperationSite`; emit only SmallInt Add or SmallInt Lt when operator, site ownership (`facts.pc == pc`), and all three exact facts match the Must-Have predicates. Every other slot is `None`. Store only Copy-able op/register data, and make the overlay immutable after construction. Treat missing/mismatched metadata conservatively as `None`; verification remains responsible for rejecting structurally invalid executables before construction.
  Must NOT do: Do not change `Instruction`, `StructureMap`, verifier, WXIR, frontend lowering, or public exports. Do not clone semantic instructions (especially vector-owning variants) or cache values/object references.

  Parallelization: Can parallel: YES | Wave 1 | Blocks: [3, 4] | Blocked by: []

  References (executor has NO interview context - be exhaustive):
  - Pattern:  `src/bytecode.rs:38-151` - public semantic instruction enum; quick code must remain distinct, and vector-owning variants explain why cloning the bytecode is forbidden.
  - API/Type: `src/structure_map.rs:4-38` - `SlotType`, `TypeFact`, and `OperationSite` exact facts and semantic-PC field.
  - API/Type: `src/structure_map.rs:59-68` - immutable operation-site table and ID lookup.
  - Pattern:  `src/wxir/builder/lowering.rs:61-93` - authoritative current Add/Lt eligibility tuples that quickening must match exactly.
  - Pattern:  `src/wxir/builder/operations.rs:22-47` - existing exact-fact/site-PC validation behavior.
  - Pattern:  `src/verifier.rs:41-65` - semantic instructions own unique verified operation-site IDs.
  - Test:     `src/wvm/equality.rs:176-193` - established private unit-test layout and Given/When/Then style.

  Acceptance criteria (agent-executable only):
  - [ ] A red-first log shows the new overlay tests failed before production definitions/logic existed; `cargo test --lib quickening` then passes after implementation.
  - [ ] Private assertions prove `quick_code.len() == executable.bytecode().code.len()` and the specialization is present at the identical semantic PC, with `None` at all other PCs.
  - [ ] Matrix assertions cover exact Add, exact Lt, Unknown for each operand/result position, wrong exact result, unsupported Binary/Compare operators, legacy typed ops, and a mismatched `facts.pc`; only the first two cases specialize.
  - [ ] A source inspection assertion/command finds no `Value`, `ObjectRef`, `ObjectHeap`, `Instruction` payload, `Vec<Register>`, or semantic-instruction clone field in the quick representation: `rg -n 'ObjectRef|ObjectHeap|Value|Vec<Register>|Instruction[[:space:]]*[>,}]|\.clone\(\)' src/wvm/quickening.rs` returns only test inputs/imports or zero matches, with each match reviewed in evidence.

  QA scenarios (MANDATORY - task incomplete without these):
  ```
  Scenario: exact WXIR-compatible sites become PC-aligned quick slots
    Tool:     bash
    Steps:    Run `set -o pipefail; cargo test --lib wvm::quickening::tests::quick_code_builds_only_exact_add_and_lt -- --exact 2>&1 | tee .omo/evidence/task-1-overlay.log`.
    Expected: The exact test passes; overlay length equals semantic code length; Add/Lt slots contain only register/op data at their original PCs.
    Evidence: .omo/evidence/task-1-overlay.log

  Scenario: unknown, wrong, and unsupported sites remain generic
    Tool:     bash
    Steps:    Run `set -o pipefail; cargo test --lib wvm::quickening::tests::quick_code_preserves_unknown_mismatched_and_unsupported_sites -- --exact 2>&1 | tee .omo/evidence/task-1-overlay-error.log`.
    Expected: The exact test passes and every negative fixture has `None` at the semantic PC without changing the input bytecode.
    Evidence: .omo/evidence/task-1-overlay-error.log
  ```

  Rollback: Apply the inverse of Task 1's recorded scoped patch only: remove `src/wvm/quickening.rs` and its single private module declaration. Do not use Git reset/checkout. If Tasks 3-5 have started, roll them back first in reverse dependency order.

  Commit: NO | Message: `N/A (caller forbids commits)` | Files: [`src/wvm/quickening.rs`, `src/wvm.rs`]

- [ ] 2. Centralize checked SmallInt addition and exact BigInt promotion

  What to do: Add a focused `pub(super)` SmallInt-add helper to `ValueOps` (or equivalently refactor the existing private Add arm) so generic arithmetic and the later quick executor share one checked-add/promotion implementation. Add the focused test first and capture red. The helper accepts raw `i64` operands, returns `Value::SmallInt` on `checked_add` success, and allocates `Object::BigInt(BigInt::from(lhs) + rhs)` through the owning heap on overflow. Keep `ValueOps::binary(Add, ...)` behavior byte-for-byte equivalent by delegating its two-SmallInt case to the helper.
  Must NOT do: Do not duplicate BigInt allocation in quickening, make arithmetic public, change sequence Add precedence, change errors, or alter other numeric operators.

  Parallelization: Can parallel: YES | Wave 1 | Blocks: [3] | Blocked by: []

  References (executor has NO interview context - be exhaustive):
  - Pattern:  `src/wvm/arithmetic.rs:24-43` - generic BinaryOperator dispatch and sequence-Add precedence that must stay unchanged.
  - Pattern:  `src/wvm/arithmetic.rs:99-109` - canonical checked SmallInt addition and BigInt promotion to extract/reuse.
  - API/Type: `src/wvm/arithmetic.rs:205-213` - heap allocation/error conversion path.
  - API/Type: `src/value.rs:4-10` - immediate SmallInt versus heap Object value representation.
  - Test:     `tests/rich_values.rs:361-416` - existing public overflow-followed-by-arithmetic semantic regression.
  - External: `https://doc.rust-lang.org/std/primitive.i64.html#method.checked_add` - checked addition contract.

  Acceptance criteria (agent-executable only):
  - [ ] A red-first helper test fails before the helper/refactor and is captured; `cargo test --lib smallint_add` passes afterward.
  - [ ] Assertions cover ordinary positive/negative addition, `i64::MAX + 1`, and `i64::MIN + (-1)`; overflow cases produce heap `Object::BigInt` with exact mathematical values and never wrap/panic.
  - [ ] `cargo test --test rich_values smallint_overflow_promotes_to_bigint_for_following_arithmetic -- --exact` remains green.
  - [ ] There remains a single production implementation of `checked_add`-to-BigInt promotion in `src/wvm/arithmetic.rs`; quickening will call it rather than copy it.

  QA scenarios (MANDATORY - task incomplete without these):
  ```
  Scenario: in-range SmallInt Add remains immediate
    Tool:     bash
    Steps:    Run `set -o pipefail; cargo test --lib wvm::arithmetic::tests::smallint_add_returns_immediate_when_in_range -- --exact 2>&1 | tee .omo/evidence/task-2-smallint-add.log`.
    Expected: The exact test passes and asserts the helper returns `Value::SmallInt(42)` for concrete in-range operands.
    Evidence: .omo/evidence/task-2-smallint-add.log

  Scenario: boundary overflow promotes exactly instead of wrapping
    Tool:     bash
    Steps:    Run `set -o pipefail; cargo test --lib wvm::arithmetic::tests::smallint_add_promotes_both_overflow_directions -- --exact 2>&1 | tee .omo/evidence/task-2-smallint-add-error.log`.
    Expected: The exact test passes; both boundary results are heap BigInts equal to `MAX+1` and `MIN-1`, with no panic or wrapped SmallInt.
    Evidence: .omo/evidence/task-2-smallint-add-error.log
  ```

  Rollback: Apply only Task 2's inverse patch, restoring the original `Number::Small + Number::Small` arm while leaving all pre-existing arithmetic edits intact. Roll back Task 3 first if it already calls the helper.

  Commit: NO | Message: `N/A (caller forbids commits)` | Files: [`src/wvm/arithmetic.rs`]

- [ ] 3. Implement side-effect-safe guarded quick execution

  What to do: Add red-first private tests, then implement one quick-execution entry point in `src/wvm/quickening.rs` with an explicit two-outcome contract such as `Handled` versus `GuardMiss`. Read operands without mutation. For quick Add, require both operands to be `Value::SmallInt`, then call Task 2's shared checked-add helper, write the result, and advance PC exactly once. For quick Lt, require both SmallInts, compare signed `i64`, write `Value::Bool`, and advance once. On either guard miss, return `GuardMiss` with PC/registers/heap unchanged so the caller can run the original instruction. Include aliasing cases (`dst == lhs`, `dst == rhs`) and a two-step overflow fixture: the first quick Add promotes, and a downstream quick slot seeing that BigInt declines cleanly.
  Must NOT do: Do not fetch or clone semantic instructions in this module, turn mismatch into an error, mutate/downgrade the slot, or advance/write before the guard and arithmetic outcome are known.

  Parallelization: Can parallel: YES | Wave 2 | Blocks: [5] | Blocked by: [1, 2]

  References (executor has NO interview context - be exhaustive):
  - API/Type: `src/wvm.rs:26-31` - frame PC/register state the quick executor may mutate only on `Handled`.
  - API/Type: `src/wvm.rs:210-228` - bounds-checked register read/write helpers to reuse.
  - Pattern:  `src/wvm/interpreter.rs:82-99` - authoritative semantic BinaryOp/CompareOp read, write, and PC behavior.
  - API/Type: `src/value.rs:4-10` - operand guards must distinguish SmallInt from Bool/Float/Object/Uninitialized.
  - Pattern:  `src/wvm/arithmetic.rs:99-109` - shared checked Add/promotion semantics after Task 2.
  - Test:     `tests/rich_values.rs:361-416` - downstream generic BigInt arithmetic behavior that guard miss must preserve.

  Acceptance criteria (agent-executable only):
  - [ ] Red logs exist for guard-miss/no-mutation and overflow/downstream-miss tests before the executor exists; `cargo test --lib quick_execution` passes after implementation.
  - [ ] Exact SmallInt Add and Lt tests assert destination and PC; aliasing tests prove original operands are read before destination replacement.
  - [ ] For Bool, Float, Object(BigInt), and Uninitialized mismatches, the outcome is `GuardMiss` and a full pre/post snapshot of PC and registers is equal; heap object count/generation is also unchanged where test-visible.
  - [ ] Overflowing quick Add returns `Handled`, advances once, and produces the exact BigInt through the shared arithmetic helper; a following quick Add/Lt with that Object returns `GuardMiss` without state change.

  QA scenarios (MANDATORY - task incomplete without these):
  ```
  Scenario: guarded Add and Lt handle exact SmallInts, including aliased destinations
    Tool:     bash
    Steps:    Run `set -o pipefail; cargo test --lib wvm::quickening::tests::quick_execution_handles_exact_smallints_and_aliases -- --exact 2>&1 | tee .omo/evidence/task-3-quick-execution.log`.
    Expected: The exact test passes; both ops write correct typed results and increment the original PC once for all alias layouts.
    Evidence: .omo/evidence/task-3-quick-execution.log

  Scenario: guard mismatch and post-overflow BigInt leave state untouched for fallback
    Tool:     bash
    Steps:    Run `set -o pipefail; cargo test --lib wvm::quickening::tests::quick_execution_guard_miss_is_side_effect_free -- --exact 2>&1 | tee .omo/evidence/task-3-quick-execution-error.log`.
    Expected: The exact test passes for Bool/Float/Object/Uninitialized and downstream BigInt operands; outcome is `GuardMiss`, PC/register snapshot is identical, and no object is allocated on the miss.
    Evidence: .omo/evidence/task-3-quick-execution-error.log
  ```

  Rollback: Apply only Task 3's inverse patch to remove the quick-execution outcome/API and its unit cases; retain the inert Task 1 overlay builder and Task 2 semantic helper. Roll back Task 5 first if it calls this API.

  Commit: NO | Message: `N/A (caller forbids commits)` | Files: [`src/wvm/quickening.rs`]

- [ ] 4. Attach quick code to verified persistent runtimes and share it through recursion

  What to do: Add `quick_code` to `FunctionRuntime` as `Arc<QuickCode>` (or `Arc<[Option<QuickInstruction>]>` if the Task 1 wrapper is zero-value), with one cache-miss constructor that builds the overlay and one cheap recursive-placeholder constructor that accepts/clones an existing Arc. Keep `verify(executable)?` before runtime removal/creation. On same-ID recursion, clone the active overlay before `mem::replace`, construct the temporary activation with that shared overlay, and preserve the existing profile/JIT/constants/current-function swap/restore semantics. Add private pointer-sharing/independence tests in a `src/wvm.rs` test module first (so Task 3 exclusively owns `quickening.rs`) and extend public lifecycle fixtures only where behavior is otherwise unobservable: same ID/clone and recursive placeholder use `Arc::ptr_eq`; distinct revision IDs do not; invalid semantic-site metadata leaves `profile_for` empty; existing A/B/A, clone, revision, and recursive profile tests remain green.
  Must NOT do: Do not add a global cache, rebuild quick code for each recursive activation, share mutable profile/JIT/constant caches, move verification after creation, or change `ExecutableId`/clone semantics.

  Parallelization: Can parallel: YES | Wave 2 | Blocks: [5] | Blocked by: [1]

  References (executor has NO interview context - be exhaustive):
  - API/Type: `src/wvm.rs:33-57` - runtime map, current `FunctionRuntime` fields, and constructor.
  - Pattern:  `src/wvm.rs:191-207` - mandatory verify-before-cache-miss ordering and runtime reinsertion by `ExecutableId`.
  - Pattern:  `src/wvm/callables.rs:67-91` - same-ID recursive runtime replacement/cache handoff to preserve exactly.
  - API/Type: `src/executable.rs:6-13` - process-local revision identity and cache-key contract.
  - API/Type: `src/executable.rs:33-80` - derived clone preserves ID; constructors create fresh IDs.
  - Test:     `tests/vm_sum.rs:98-184` - revision, A/B/A, clone, and invalid-runtime lifecycle patterns.
  - Test:     `tests/call_depth.rs:69-88` - recursive profile preservation through the current handoff.
  - Test:     `tests/runtime_api.rs:175-240` - public repeated-clone and independent-runtime persistence.
  - External: `https://doc.rust-lang.org/std/sync/struct.Arc.html` - `Arc::clone` shared allocation and `Arc::ptr_eq` identity semantics.

  Acceptance criteria (agent-executable only):
  - [ ] Red-first pointer/lifecycle tests are captured; `cargo test --lib quick_code_runtime` and the named existing integration tests pass after implementation.
  - [ ] Private assertions prove top-level cache-miss runtime owns the built overlay, a same-ID recursive placeholder is `Arc::ptr_eq`, and a fresh executable revision gets a different allocation.
  - [ ] `verify(executable)?` remains before `.runtimes.remove(...)` and every quick-code-building constructor call; a malformed operation-site executable returns an error and `profile_for(&invalid).is_none()`.
  - [ ] Existing `clone_preserves_identity_and_reuses_runtime`, `a_b_a_reuses_each_executables_compiled_runtime`, `executable_revisions_have_independent_identity_and_runtime_state`, and `same_function_nested_activations_preserve_runtime_profile_updates` tests pass unchanged or with only focused quick-code assertions.
  - [ ] Recursive placeholder creation performs only an `Arc::clone` for quick code; no overlay traversal/construction appears in `src/wvm/callables.rs`.

  QA scenarios (MANDATORY - task incomplete without these):
  ```
  Scenario: clone, A/B/A, revision, and recursion use the correct overlay ownership
    Tool:     bash
    Steps:    Run `set -o pipefail; cargo test --lib wvm::tests::quick_code_runtime_identity -- --exact 2>&1 | tee .omo/evidence/task-4-runtime-lifecycle.log`; then run these exact commands: `cargo test --test vm_sum clone_preserves_identity_and_reuses_runtime -- --exact`, `cargo test --test vm_sum a_b_a_reuses_each_executables_compiled_runtime -- --exact`, `cargo test --test vm_sum executable_revisions_have_independent_identity_and_runtime_state -- --exact`, and `cargo test --test call_depth same_function_nested_activations_preserve_runtime_profile_updates -- --exact`.
    Expected: Unit pointer assertions pass; clone/recursive placeholder share one overlay allocation; A and B/revisions retain independent runtimes across A/B/A; all public results/profile/JIT assertions remain green.
    Evidence: .omo/evidence/task-4-runtime-lifecycle.log

  Scenario: verifier-invalid executable never populates runtime or quick code
    Tool:     bash
    Steps:    Run `set -o pipefail; cargo test --lib wvm::tests::invalid_executable_builds_no_quick_runtime -- --exact 2>&1 | tee .omo/evidence/task-4-runtime-lifecycle-error.log`; then run `cargo test --test vm_sum invalid_executable_does_not_disturb_a_cached_runtime -- --exact`.
    Expected: Both tests pass; execution returns the verifier error, invalid `ExecutableId` is absent from runtime/profile lookup, and the prior valid runtime remains reusable.
    Evidence: .omo/evidence/task-4-runtime-lifecycle-error.log
  ```

  Rollback: After rolling back Task 5, apply Task 4's inverse patch only: remove the `quick_code` field/constructors/import, restore the original cache-miss constructor, and restore the exact prior `mem::replace(runtime, FunctionRuntime::new(caller))` handoff. Never reset the dirty files wholesale.

  Commit: NO | Message: `N/A (caller forbids commits)` | Files: [`src/wvm.rs`, `src/wvm/callables.rs`]

- [ ] 5. Wire post-JIT quick dispatch and prove semantic/JIT/lifecycle fidelity end to end

  What to do: Add `tests/static_quickening.rs` first with controlled semantic bytecode/StructureMap fixtures and capture red. In `execute_with_runtime`, preserve profiling and call `try_execute_region` first. Only when it returns `false`, look up the current PC's quick slot and call Task 3's executor; `Handled` continues the loop, while `GuardMiss` immediately proceeds to fetch and execute the original `function.code[pc]`. Add no other dispatch sites. Cover: exact Add/Lt; Unknown/unsupported behavior; intentional exact-fact/runtime-type mismatch; bytecode snapshot equality before/after first and repeated execution; quick overflow to BigInt; downstream BigInt semantic fallback; clone/A-B/A/revision persistence; same-function recursion; and a semantic `BinaryOp(Add)` hot loop whose compiled overflow returns `ReplayInstruction`, resumes at the identical PC, and promotes correctly when post-JIT quick dispatch runs. Assert WXIR/JIT still consumes the semantic `BinaryOp` and `StructureMap`, not quick code. Mark only the native-code JIT fixture `#[cfg_attr(miri, ignore)]`; all interpreter-only quickening cases must remain executable under Miri.
  Must NOT do: Do not call quick dispatch before JIT, inside JIT/WXIR, after already executing semantic code, or from verifier/source mapping. Do not weaken JIT report assertions or replace the original semantic replay fixture with legacy `AddI64`.

  Parallelization: Can parallel: NO | Wave 3 | Blocks: [F1, F2, F3, F4] | Blocked by: [3, 4]

  References (executor has NO interview context - be exhaustive):
  - Pattern:  `src/wvm/interpreter.rs:16-52` - exact dispatch loop and only permitted insertion point: after line 35 returns false and before semantic lookup at line 38.
  - Pattern:  `src/wvm/interpreter.rs:82-99` - original semantic fallback that must remain unchanged.
  - Pattern:  `src/wvm/jit_runtime.rs:41-105` - JIT attempt/decline/compile priority that quick dispatch must not bypass.
  - Pattern:  `src/wvm/jit_runtime.rs:121-136` - replay PC, one-iteration OSR suppression, and cached-entry mismatch fallback.
  - API/Type: `src/wxir/ir.rs:270-278` - `ReplayInstruction` requires interpreter replay at `resume_pc`.
  - Pattern:  `src/wxir/builder/lowering.rs:39-100` - WXIR reads semantic bytecode and exact facts; leave untouched.
  - Test:     `tests/jit_region.rs:253-329` - existing native overflow/no-destination-write and interpreter replay assertions to mirror with semantic `BinaryOp`.
  - Test:     `tests/python_frontend.rs:20-101` - semantic PCs, exact Add/Lt facts, interpreter/tiered equivalence, and repeated-runtime behavior.
  - Test:     `tests/rich_values.rs:9-40` - fixture style for semantic operation sites; its Unknown default is useful only for fallback cases, so the new fixture must accept explicit facts.
  - Test:     `tests/rich_values.rs:361-416` - exact promoted value after downstream arithmetic.
  - Test:     `tests/call_depth.rs:69-88` - existing recursive source already contains exact SmallInt Lt/Add sites and exercises shared recursive quick code once dispatch is live.
  - External: `https://github.com/rust-lang/miri/` - official Miri installation/command guide; Tree Borrows is enabled with `MIRIFLAGS="-Zmiri-tree-borrows"`.

  Acceptance criteria (agent-executable only):
  - [ ] `tests/static_quickening.rs` is red before the hook and green after; `cargo test --test static_quickening` passes all named Given/When/Then cases.
  - [ ] Source inspection shows exactly one quick-dispatch call in production, lexically after `try_execute_region` and before `.code.get(frame.pc)`; verifier, WXIR, JIT, `StructureMap`, `Instruction`, and `ExecutableFunction` have no plan-owned changes.
  - [ ] Exact Add/Lt return the same values as semantic execution; Unknown/unsupported/wrong-runtime-type cases execute the generic instruction and preserve its result/error behavior.
  - [ ] `let before = executable.bytecode().clone()` remains equal to `executable.bytecode()` after first execution, repeated execution, clone execution, JIT replay, and BigInt fallback; semantic variants and operation-site PCs are unchanged.
  - [ ] Overflow case yields exact BigInt `i64::MAX + 1`; a downstream Add with that BigInt yields exact `i64::MAX + 2` through semantic fallback.
  - [ ] Semantic JIT fixture reports one native `ReplayInstruction` at the original `BinaryOp(Add)` PC, leaves destination uncommitted before replay, then completes with BigInt; a subsequent compatible execution reuses the compiled region.
  - [ ] Clone/A-B/A/revision/invalid/recursive tests required by Task 4 and all pre-existing 111 tests remain green.

  QA scenarios (MANDATORY - task incomplete without these):
  ```
  Scenario: exact static quickening executes while semantic bytecode remains immutable
    Tool:     bash
    Steps:    Run `set -o pipefail; cargo test --test static_quickening exact_add_and_lt_execute_without_mutating_semantic_bytecode -- --exact 2>&1 | tee .omo/evidence/task-5-dispatch.log`.
    Expected: The exact test passes with correct SmallInt/Bool outputs; pre/post semantic bytecode and StructureMap snapshots are equal and operation-site PCs are unchanged.
    Evidence: .omo/evidence/task-5-dispatch.log

  Scenario: overflow replay promotes, downstream BigInt falls back, and JIT remains first
    Tool:     bash
    Steps:    Run `set -o pipefail; cargo test --test static_quickening semantic_jit_replay_promotes_then_falls_back_for_bigint -- --exact 2>&1 | tee .omo/evidence/task-5-dispatch-error.log`.
    Expected: The exact test passes; JIT report is `ReplayInstruction` at the semantic Add PC, quick checked Add produces `MAX+1` BigInt, the next quick guard declines without mutation, generic Add returns `MAX+2`, and compiled runtime remains reusable.
    Evidence: .omo/evidence/task-5-dispatch-error.log
  ```

  Rollback: Apply Task 5's inverse patch only: remove the single interpreter hook/import and `tests/static_quickening.rs`. Tasks 1-4 then remain inert private/runtime data; to fully revert the feature, continue Tasks 4, 3, 2, 1 in that order. Never restore whole files from Git because they contain user changes.

  Commit: NO | Message: `N/A (caller forbids commits)` | Files: [`src/wvm/interpreter.rs`, `tests/static_quickening.rs`]

## Final verification wave (MANDATORY - after all implementation tasks)
> Runs in PARALLEL. ALL must APPROVE. Surface results to the caller and wait for an explicit "okay" before declaring complete.
- [ ] F1. Plan compliance audit - every task done, every acceptance criterion met.

  ```
  Scenario: required evidence, LOC ceiling, and protected semantic files are compliant
    Tool:     bash
    Steps:    Run `set -o pipefail; { for file in task-1-overlay.log task-1-overlay-error.log task-2-smallint-add.log task-2-smallint-add-error.log task-3-quick-execution.log task-3-quick-execution-error.log task-4-runtime-lifecycle.log task-4-runtime-lifecycle-error.log task-5-dispatch.log task-5-dispatch-error.log final-pure-loc.txt; do test -s ".omo/evidence/$file"; done; awk -F': ' '/^total_added_pure_loc:/ { found=1; if ($2 > 250) exit 1 } END { if (!found) exit 1 }' .omo/evidence/final-pure-loc.txt; for path in Cargo.toml Cargo.lock src/bytecode.rs src/executable.rs src/structure_map.rs src/verifier.rs src/verifier/structure.rs src/frontend/python/lower/expression.rs src/wvm/jit_runtime.rs src/wxir/builder/setup.rs src/wxir/builder/lowering.rs src/wxir/builder/operations.rs src/wxir/ir.rs; do cmp -s ".omo/evidence/baseline/$path" "$path" || { echo "protected file changed: $path"; exit 1; }; done; } 2>&1 | tee .omo/evidence/f1-plan-compliance.log`.
    Expected: Exit 0; all task evidence is nonempty, `total_added_pure_loc` exists and is <=250, and every protected semantic/JIT/WXIR file is byte-identical to its baseline.
    Evidence: .omo/evidence/f1-plan-compliance.log
  ```

- [ ] F2. Code quality review - diagnostics clean, idioms match, no dead code.

  ```
  Scenario: formatting, lint, and baseline-isolated whitespace checks are clean
    Tool:     bash
    Steps:    Run `set -o pipefail; { cargo fmt --all -- --check; cargo clippy --all-targets -- -D warnings; for path in src/wvm.rs src/wvm/arithmetic.rs src/wvm/callables.rs src/wvm/interpreter.rs src/wvm/quickening.rs tests/static_quickening.rs; do check_status=0; : > .omo/evidence/f2-whitespace.tmp; git diff --no-index --check ".omo/evidence/baseline/$path" "$path" > .omo/evidence/f2-whitespace.tmp 2>&1 || check_status=$?; if [ "$check_status" -gt 1 ] || [ -s .omo/evidence/f2-whitespace.tmp ]; then cat .omo/evidence/f2-whitespace.tmp; echo "whitespace check failed: $path"; exit 1; fi; echo "clean whitespace: $path (diff status $check_status accepted)"; done; } 2>&1 | tee .omo/evidence/f2-code-quality.log`.
    Expected: Exit 0; rustfmt reports no changes, Clippy emits no warnings, and each plan-owned file prints `clean whitespace`. `git diff --no-index` status 1 is explicitly accepted only when its `--check` output is empty (ordinary clean differences); status >1 or any whitespace diagnostic fails the scenario. Review output also confirms no dead quick variant, unsafe block, or cached `ObjectRef`.
    Evidence: .omo/evidence/f2-code-quality.log
  ```

- [ ] F3. Real manual QA - every QA scenario executed with evidence captured.

  ```
  Scenario: targeted, full-suite, and available Tree-Borrows tests pass
    Tool:     bash
    Steps:    Run `set -o pipefail; { cargo test --test static_quickening; cargo test --test call_depth; cargo test --test jit_region; cargo test --all-targets; if rustup toolchain list | grep -Eq '^nightly' && rustup component list --toolchain nightly --installed | grep -Eq '^miri'; then MIRIFLAGS="-Zmiri-tree-borrows" cargo +nightly miri test --lib quickening; MIRIFLAGS="-Zmiri-tree-borrows" cargo +nightly miri test --test static_quickening; else echo "INFRASTRUCTURE_BLOCKED: pre-existing nightly/Miri unavailable; no installation attempted" | tee .omo/evidence/f3-miri-blocked.log; exit 1; fi; } 2>&1 | tee .omo/evidence/f3-real-qa.log`.
    Expected: Exit 0; focused suites and `cargo test --all-targets` pass, and both relevant Tree-Borrows runs pass. If nightly/Miri is absent, the command exits 1 with explicit blocked evidence; F3 must not approve, and no installation/network command is attempted.
    Evidence: .omo/evidence/f3-real-qa.log (or .omo/evidence/f3-miri-blocked.log for the explicit blocker)
  ```

- [ ] F4. Scope fidelity - nothing extra shipped beyond Must-Have, nothing Must-NOT-Have introduced.

  ```
  Scenario: unrelated dirty state is preserved and only the authorized specialization surface changed
    Tool:     bash
    Steps:    Run `set -o pipefail; { awk '$0 !~ / (src\/wvm.rs|src\/wvm\/arithmetic.rs|src\/wvm\/callables.rs|src\/wvm\/interpreter.rs|src\/wvm\/quickening.rs|tests\/static_quickening.rs)$/' .omo/evidence/baseline-status.txt | sort > .omo/evidence/f4-baseline-unrelated.txt; git status --short | awk '$0 !~ / (src\/wvm.rs|src\/wvm\/arithmetic.rs|src\/wvm\/callables.rs|src\/wvm\/interpreter.rs|src\/wvm\/quickening.rs|tests\/static_quickening.rs)$/' | sort > .omo/evidence/f4-final-unrelated.txt; diff -u .omo/evidence/f4-baseline-unrelated.txt .omo/evidence/f4-final-unrelated.txt; cargo test --test static_quickening; rg -n 'ObjectRef|unsafe|AddI64|LtI64' src/wvm/quickening.rs || true; } 2>&1 | tee .omo/evidence/f4-scope-fidelity.log`.
    Expected: Exit 0; unrelated status entries are identical, the full static-quickening contract suite passes (including exactly Add/Lt specialization and all unsupported fallbacks), and any final search matches are confined to negative test fixtures—not production quick-code fields or implementations.
    Evidence: .omo/evidence/f4-scope-fidelity.log
  ```

## Commit strategy
- No commits are authorized for this execution. Keep all plan-owned edits in the working tree and report the exact scoped file list.
- Preserve atomic patch/evidence boundaries per task so each task can be reviewed or inversely applied without touching the user's existing changes.
- Do not run `git add`, `git commit`, `git reset`, `git checkout`, `git clean`, or rewrite history.
- If the caller later authorizes commits, use one logical Conventional Commit per atomic task and include `Plan: .omo/plans/static-quickening.md` in the final commit footer; that later authorization is outside this plan.

## Success criteria
- All Must-Have shipped; all QA scenarios pass with captured evidence; F1-F4 approved; no commits created; pre-existing working-tree changes preserved.
