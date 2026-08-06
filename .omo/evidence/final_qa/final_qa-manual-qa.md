# Wustite VM manual QA

Executed from `/home/entity27th/Wustite` on 2026-08-06. Commands use the repository's public Cargo test/CLI surfaces; temporary Python fixtures were written only under `/tmp`.

## manualQa

### surfaceEvidence

| scenario id | criterion reference | surface | exact invocation | verdict | artifactRefs |
|---|---|---|---|---|---|
| S1 | C1 function argument/execution ABI | Public `Runtime::compile_function` + `Runtime::execute_with_args` | `cargo test --test runtime_api typed_positional_arguments_cross_the_execution_abi -- --nocapture` | PASS | A01 |
| S2 | C2 SmallInt/Float/Bool and ObjectRef-backed String/Tuple/BigInt/List/Dict/Function | Public Runtime/WVM rich-value execution tests | `cargo test --test python_rich_values -- --nocapture` | PASS | A02 |
| S3 | C1, C2 CLI typed scalar and object parsing | CLI `run` with `add.py` and `rich_arguments.py` | `cargo run --quiet -- run examples/add.py --function add --arg 20 --arg 22`; `cargo run --quiet -- run tests/fixtures/rich_arguments.py --function string_echo --arg wustite --interpreter --json`; `cargo run --quiet -- run tests/fixtures/rich_arguments.py --function bigint_echo --arg 9223372036854775808 --interpreter --json`; `cargo run --quiet -- run tests/fixtures/rich_arguments.py --function bool_echo --arg true --interpreter --json`; `cargo run --quiet -- run tests/fixtures/rich_arguments.py --function float_echo --arg 2.5 --interpreter --json` | PASS | A03, A04, A05, A06, A07 |
| S4 | C3 overflow promotion in interpreter | CLI `run --interpreter --json` on a temporary boundary fixture | `cargo run --quiet -- run /tmp/wustite_overflow.py --interpreter --json`; `cargo run --quiet -- run /tmp/wustite_underflow.py --interpreter --json` | PASS | A08, A09 |
| S5 | C3 exact numeric boundary value | Interpreter computes equality against `2^63` and `-(2^63)-1` | `cargo run --quiet -- run /tmp/wustite_overflow_check.py --interpreter --json`; `cargo run --quiet -- run /tmp/wustite_underflow_check.py --interpreter --json` | PASS (both JSON results `bool: true`) | A10, A11 |
| S6 | C4 interpreter/JIT-facing overflow replay | Adaptive CLI loop tiers up, exits before committing SmallInt overflow, replays in WVM | `cargo run --quiet -- run /tmp/wustite_overflow.py --hot-threshold 1 --trace-jit` | PASS (`kind=big_int`, `last_exit_kind=replay_instruction`) | A12 |
| S6b | C4 interpreter/JIT-facing underflow fallback | Adaptive CLI subtract loop remains semantically correct when native builder declines unsupported subtraction | `cargo run --quiet -- run /tmp/wustite_underflow.py --hot-threshold 1 --trace-jit` | PASS with fallback caveat (`kind=big_int`; region disabled with `unsupported WVM instruction BinaryOp::Subtract`) | A23 |
| S7 | C1 controlled invalid-type/arity errors | CLI argument parser and ABI error boundary | `cargo run --quiet -- run examples/add.py --function add --arg nope --arg 22`; `cargo run --quiet -- run examples/add.py --function add --arg 20`; `cargo run --quiet -- run /tmp/wustite_tuple.py --function tuple_echo --arg x` | PASS (all exit 1 with typed diagnostics) | A13, A14, A15 |
| S8 | C5 stale/foreign ObjectRef rejection | Object heap public handles | `cargo test --test object_heap stale_handles_are_rejected_after_remove_and_slot_reuse -- --nocapture`; `cargo test --test object_invariants -- --nocapture` | PASS | A16, A17 |
| S9 | C6 recursion/cycle controlled errors | Guest recursion depth and function-reference cycle handling | `cargo test --test call_depth direct_guest_recursion_returns_an_execution_error_at_the_call_depth_limit -- --nocapture`; `cargo test --test python_function_identity function_reference_cycles_remain_rejected -- --nocapture` | PASS | A18, A19 |
| S10 | C7 numeric semantic safety | Exact mixed numeric comparison and controlled NaN/huge conversion errors | `cargo test --test numeric_semantics -- --nocapture` | PASS (11/11) | A20 |
| S11 | C1–C7 regression gate | Full repository test surface | `cargo test --all --all-targets` | PASS (all listed tests passed) | A21 |

### adversarialCases

| scenario id | criterion reference | adversarial class | expected behavior | verdict | artifactRefs |
|---|---|---|---|---|---|
| ADV1 | C1 | wrong ABI type | Reject before execution with parameter name and expected/actual type | PASS | A13 |
| ADV2 | C1 | wrong ABI arity | Reject before execution with expected/provided positional counts | PASS | A14 |
| ADV3 | C2 | CLI object type not constructible from text (tuple) | Controlled usage error directs caller to Runtime API | PASS | A15 |
| ADV4 | C5 | stale handle after remove/slot reuse | Reject stale generation/heap reference; no use-after-free | PASS | A16, A17 |
| ADV5 | C6 | unbounded guest recursion | Return typed guest call-depth error instead of host-stack exhaustion | PASS | A18 |
| ADV6 | C6 | recursive function-reference cycle at compile time | Reject with controlled cycle diagnostic | PASS | A19 |
| ADV7 | C3 | positive i64 overflow (`i64::MAX + 1`) | Promote to BigInt, preserving exact value | PASS | A08, A10, A12 |
| ADV8 | C3 | negative i64 overflow (`i64::MIN - 1`) | Promote to BigInt, preserving exact value | PASS | A09, A11 |
| ADV9 | C4 | JIT overflow side exit | Native region must side-exit/replay without committing wrapped SmallInt | PASS | A12, A22 |
| ADV10 | C7 | NaN ordering / huge BigInt↔Float conversion | Return controlled numeric error, never infinity/undefined result | PASS | A20 |
| ADV11 | C4 | Adaptive negative overflow with currently unsupported native subtract | Must return exact BigInt through interpreter fallback, without an execution error | PASS with fallback caveat | A23 |

## artifactRefs

| id | kind | description | path |
|---|---|---|---|
| A01 | test transcript | Typed positional argument ABI success | `/home/entity27th/Wustite/.omo/evidence/final_qa/abi-success-test.log` |
| A02 | test transcript | 10 Python rich-value scenarios: Float, Bool, String, BigInt, Tuple, List, Dict, Function | `/home/entity27th/Wustite/.omo/evidence/final_qa/python-rich-values-test.log` |
| A03 | CLI transcript | `add(20,22)` prints `42` | `/home/entity27th/Wustite/.omo/evidence/final_qa/cli-add.log` |
| A04 | CLI transcript | String ObjectRef JSON snapshot | `/home/entity27th/Wustite/.omo/evidence/final_qa/cli-string-json.log` |
| A05 | CLI transcript | BigInt ObjectRef JSON snapshot | `/home/entity27th/Wustite/.omo/evidence/final_qa/cli-bigint-json.log` |
| A06 | CLI transcript | Bool JSON snapshot | `/home/entity27th/Wustite/.omo/evidence/final_qa/cli-bool-json.log` |
| A07 | CLI transcript | Float JSON snapshot | `/home/entity27th/Wustite/.omo/evidence/final_qa/cli-float-json.log` |
| A08 | CLI transcript | Positive overflow returns `kind=big_int` | `/home/entity27th/Wustite/.omo/evidence/final_qa/cli-overflow-interpreter-json.log` |
| A09 | CLI transcript | Negative overflow returns `kind=big_int` | `/home/entity27th/Wustite/.omo/evidence/final_qa/cli-underflow-interpreter-json.log` |
| A10 | CLI transcript | Exact positive BigInt equality check (`true`) | `/home/entity27th/Wustite/.omo/evidence/final_qa/cli-overflow-exact-check.log` |
| A11 | CLI transcript | Exact negative BigInt equality check (`true`) | `/home/entity27th/Wustite/.omo/evidence/final_qa/cli-underflow-exact-check.log` |
| A12 | CLI transcript | Adaptive JIT overflow replay (`replay_instruction`) | `/home/entity27th/Wustite/.omo/evidence/final_qa/cli-overflow-adaptive.log` |
| A13 | CLI transcript | Invalid scalar type diagnostics and exit=1 | `/home/entity27th/Wustite/.omo/evidence/final_qa/cli-invalid-int-status.log` |
| A14 | CLI transcript | Wrong arity diagnostic and exit=1 | `/home/entity27th/Wustite/.omo/evidence/final_qa/cli-wrong-arity-status.log` |
| A15 | CLI transcript | Unsupported tuple CLI diagnostic and exit=1 | `/home/entity27th/Wustite/.omo/evidence/final_qa/cli-unsupported-object-arg-status.log` |
| A16 | test transcript | Stale heap handle rejection after slot reuse | `/home/entity27th/Wustite/.omo/evidence/final_qa/stale-ref-test.log` |
| A17 | test transcript | Foreign/stale nested container refs and invalid object invariants | `/home/entity27th/Wustite/.omo/evidence/final_qa/object-invariants-test.log` |
| A18 | test transcript | Guest recursion depth limit | `/home/entity27th/Wustite/.omo/evidence/final_qa/recursion-limit-test.log` |
| A19 | test transcript | Recursive function-reference cycle rejection | `/home/entity27th/Wustite/.omo/evidence/final_qa/function-cycle-test.log` |
| A20 | test transcript | Numeric semantics and controlled error cases (11/11) | `/home/entity27th/Wustite/.omo/evidence/final_qa/numeric-semantics-test.log` |
| A21 | test transcript | Full `cargo test --all --all-targets` regression run | `/home/entity27th/Wustite/.omo/evidence/final_qa/cargo-test-all.log` |
| A22 | test transcript | Synthetic JIT overflow side-exit replay | `/home/entity27th/Wustite/.omo/evidence/final_qa/jit-overflow-replay-test.log` |
| A23 | CLI transcript | Adaptive underflow fallback; exact BigInt plus JIT disable reason | `/home/entity27th/Wustite/.omo/evidence/final_qa/cli-underflow-adaptive.log` |
