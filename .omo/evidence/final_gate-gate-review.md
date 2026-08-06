# WVM ISA/runtime expansion — final gate review

- `recommendation`: **APPROVE**
- `blockers`: None.

## originalIntent

Expand WVM with a typed function/execution ABI; public `SmallInt`, `Float`, and
`Bool` values; ObjectRef-backed String, Tuple, BigInt, List, Dict, and Function
values; and exact BigInt promotion for every overflowing i64 arithmetic path.
Where native checked arithmetic exists, overflow must exit before committing a
wrapped value and replay in the interpreter. The public `I64` to `SmallInt`
rename is intentional. Interpreter-only rich operations and native subtraction
fallback are acceptable when execution still returns the exact BigInt without
an overflow error.

## desiredOutcome

Callers can compile and invoke typed functions with typed values, execute rich
heap-backed values safely, and observe exact BigInt results rather than i64
overflow errors or wrapped values in interpreter and adaptive/JIT execution.

## userOutcomeReview

The shipped tree satisfies the requested outcome. The public runtime ABI tests
exercise typed positional arguments and controlled type/arity failures. Rich
runtime tests cover every requested scalar/object family. `ValueOps` uses
`checked_add`, `checked_sub`, `checked_mul`, and `checked_neg`, allocating an
Object-backed BigInt on every SmallInt overflow. Native checked addition exits
with `ReplayInstruction` before restoring the overflowing destination; the VM
then replays the instruction and returns the exact BigInt. Unsupported native
subtraction disables the region and completes through the interpreter with an
exact BigInt, matching the explicitly accepted limitation.

The public rename is documented in `README.md`; legacy bytecode and internal
machine-width `I64` names remain intentionally distinct from the public Value
variant.

## Reproduced evidence

- `cargo test --all --all-targets`: PASS, 111 tests.
- `cargo test --test runtime_api typed_positional_arguments_cross_the_execution_abi -- --nocapture`: PASS.
- `cargo test --test python_rich_values -- --nocapture`: PASS, 10 tests.
- `cargo test --test numeric_semantics -- --nocapture`: PASS, 11 tests.
- `cargo fmt --all -- --check`: PASS.
- `cargo clippy --all-targets --all-features -- -D warnings`: PASS.
- `git diff --check`: PASS.
- Adaptive checked-add transcript returns `kind=big_int`, with one compiled
  region/native execution and `last_exit_kind=replay_instruction`.
- Adaptive subtraction transcript returns `kind=big_int`, with the region
  disabled for the documented unsupported `BinaryOp::Subtract` lowering.
- Synthetic replay transcript passes
  `vm_replays_synthetic_overflow_exit_in_interpreter`.

## Required independent skill-perspective pass

### remove-ai-slops / overfit

Directly reviewed the production arithmetic/replay code and relevant tests.
Found no deletion-only tests, tests that merely assert a requested removal,
prose/prompt pins, tautological expected values, or tests that only mirror an
implementation constant. The overflow tests assert observable exact values,
unmodified replay state, exit metadata, and public execution results. The
production module splits correspond to concrete responsibilities; no needless
parsing/normalization or speculative extraction violates a success criterion.

### programming

The requested public type distinction is encoded in enums and typed ABI
metadata. Arithmetic matches are exhaustive. The native unsafe call remains
inside a narrow safe wrapper with ABI/lifetime/layout safety justification.
Clippy and formatting gates pass. Test-only `unwrap` use is not a production
finding. No maintenance, false-confidence, or scope-drift issue violates a
stated success criterion.

The independent code-review report explicitly contains both the programming
and remove-ai-slops perspectives and explicitly covers deletion-only,
requested-removal, tautological, prose, and implementation-mirroring tests.
This direct gate pass independently reaches the same conclusion.

## Checked artifacts

- `.omo/evidence/final_code_review-code-review.md`
- `.omo/evidence/final_qa/final_qa-manual-qa.md`
- `.omo/evidence/final_qa/abi-success-test.log`
- `.omo/evidence/final_qa/python-rich-values-test.log`
- `.omo/evidence/final_qa/cli-overflow-interpreter-json.log`
- `.omo/evidence/final_qa/cli-underflow-interpreter-json.log`
- `.omo/evidence/final_qa/cli-overflow-exact-check.log`
- `.omo/evidence/final_qa/cli-underflow-exact-check.log`
- `.omo/evidence/final_qa/cli-overflow-adaptive.log`
- `.omo/evidence/final_qa/cli-underflow-adaptive.log`
- `.omo/evidence/final_qa/jit-overflow-replay-test.log`
- `.omo/evidence/final_qa/cargo-test-all.log`
- `src/wvm/arithmetic.rs`
- `src/jit/compiled_region.rs`
- `src/wxir/builder/lowering.rs`
- `tests/rich_values.rs`
- `tests/jit_region.rs`
- `tests/runtime_api.rs`
- `tests/python_rich_values.rs`
- `tests/numeric_semantics.rs`
- `README.md`

## exactEvidenceGaps

- `omo ulw-loop status --json` cannot execute because `node` is unavailable;
  therefore the mandated fallback path `.omo/evidence/final_gate-gate-review.md`
  is used.
- Generated Cranelift executable memory cannot be exercised under Miri. This is
  not a criterion failure: the real native test suite and adaptive CLI evidence
  reproduce the checked-add side exit and exact interpreter replay.
- Explicit boundary tests focus on add/replay and subtract/fallback. Multiply
  and negate overflow promotion are additionally established by direct source
  inspection of their checked operations and common BigInt allocation path;
  no contradictory behavior or overflow error path exists.

