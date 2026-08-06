# Static quickening manual QA

Executed from `/home/entity27th/Wustite` on 2026-08-06. No production files were modified by this QA pass.

## manualQa

### surfaceEvidence

| scenario id | criterion reference | surface | exact invocation | verdict | artifactRefs |
|---|---|---|---|---|---|
| SQ-1 | exact SmallInt Add/Lt quick execution | WVM unit quick executor | `cargo test quickening -- --nocapture` | PASS (4/4) | SQ-A1 |
| SQ-2 | semantic bytecode and StructureMap unchanged; repeated execution and same-ID clone reuse | Public `Vm::execute` and immutable `ExecutableFunction` | `cargo test --test static_quickening exact_add_and_lt_execute_without_mutating_semantic_bytecode -- --nocapture`; `cargo test --test vm_sum clone_preserves_identity_and_reuses_runtime -- --nocapture`; `cargo test --test runtime_api repeated_execute_and_clone_reuse_native_region -- --nocapture` | PASS | SQ-A2, SQ-A3, SQ-A7 |
| SQ-3 | exact-fact runtime type mismatch falls back to float semantics; unknown/unsupported sites remain semantic | Public WVM execution | `cargo test --test static_quickening unknown_unsupported_and_runtime_mismatch_use_semantic_behavior -- --nocapture` | PASS (returns `Float(4.0)`) | SQ-A2 |
| SQ-4 | positive and negative i64 overflow promote to BigInt | WVM arithmetic unit semantics | `cargo test smallint_add_promotes_both_overflow_directions -- --nocapture` | PASS | SQ-A4 |
| SQ-5 | native semantic Add replay at original PC promotes; downstream quick BigInt guard-misses to generic semantics | Adaptive WVM/JIT replay | `cargo test --test static_quickening semantic_jit_replay_promotes_then_falls_back_for_bigint -- --nocapture` | PASS (BigInt result, `last_resume_pc=3`, `ReplayInstruction`, second run reuses region) | SQ-A2 |
| SQ-6 | recursive same-ID calls preserve runtime behavior/profile | Guest recursion + cached runtime | `cargo test --test call_depth -- --nocapture`; `cargo test --test vm_sum a_b_a_reuses_each_executables_compiled_runtime -- --nocapture` | PASS | SQ-A5, SQ-A3 |
| SQ-7 | regression gate | All Rust targets | `cargo test --all-targets -- --nocapture` | PASS (all listed tests passed) | SQ-A6 |

### adversarialCases

| scenario id | criterion reference | adversarial class | expected behavior | verdict | artifactRefs |
|---|---|---|---|---|---|
| SQ-ADV1 | exact quick execution | SmallInt operand mismatch (`Bool`, `Float`, `BigInt`, `Uninitialized`) | Guard-miss leaves PC/registers/heap unchanged | PASS | SQ-A1 |
| SQ-ADV2 | overflow safety | positive and negative i64 boundary overflow | Promote exactly to heap BigInt; never wrap | PASS | SQ-A4 |
| SQ-ADV3 | quick/JIT interaction | quick Add overflow followed by quick Lt reading BigInt | First operation handles with promotion; downstream quick site guard-misses and generic semantics resumes | PASS | SQ-A1, SQ-A2 |
| SQ-ADV4 | semantic preservation | exact static facts with runtime Float values | Do not execute SmallInt quick path; generic Float operation returns exact float | PASS | SQ-A2 |
| SQ-ADV5 | unsupported operation handling | Subtract and unknown facts | Remain semantic bytecode and execute without quickening | PASS | SQ-A2 |
| SQ-ADV6 | runtime identity | same-ID clone and A-B-A execution reuse | Reuse one cached runtime/quick code for clone; keep independent revisions isolated | PASS | SQ-A3 |
| SQ-ADV7 | recursive activation safety | nested same-function calls and depth exhaustion | Preserve profile/runtime state; controlled depth error instead of host failure | PASS | SQ-A5 |

### artifactRefs

| id | kind | description | path |
|---|---|---|---|
| SQ-A1 | test transcript | Four quickening construction/execution unit tests, including side-effect-free guard misses and overflow/downstream guard | `/home/entity27th/Wustite/.omo/evidence/final_qa/static_quickening/quickening_unit.log` |
| SQ-A2 | test transcript | Three static quickening integration tests: exact Add/Lt immutability, mismatch/unsupported semantics, JIT replay BigInt fallback | `/home/entity27th/Wustite/.omo/evidence/final_qa/static_quickening/static_quickening.log` |
| SQ-A3 | test transcript | VM runtime identity, clone reuse, A-B-A cache behavior, and revision isolation | `/home/entity27th/Wustite/.omo/evidence/final_qa/static_quickening/runtime_identity.log` |
| SQ-A4 | test transcript | Both SmallInt overflow directions promote to BigInt | `/home/entity27th/Wustite/.omo/evidence/final_qa/static_quickening/overflow_directions.log` |
| SQ-A5 | test transcript | Three guest recursion/depth scenarios pass | `/home/entity27th/Wustite/.omo/evidence/final_qa/static_quickening/recursion.log` |
| SQ-A6 | test transcript | Full `cargo test --all-targets -- --nocapture` regression run | `/home/entity27th/Wustite/.omo/evidence/final_qa/static_quickening/full_all_targets.log` |
| SQ-A7 | test transcript | Public Runtime repeated execution and same-ID clone reuse of a compiled native region | `/home/entity27th/Wustite/.omo/evidence/final_qa/static_quickening/runtime_api_clone.log` |
