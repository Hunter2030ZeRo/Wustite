# Manual QA — quickening `#[inline]` final fix

## surfaceEvidence

| scenario id | criterion reference | surface | exact invocation | verdict | artifactRefs |
|---|---|---|---|---|---|
| QF-SEM-01 | semantic quickening correctness | release Rust integration test | `taskset -c 1 cargo test --release --test static_quickening -- --nocapture` | PASS | `sem` |
| QF-SEM-02 | quick-code construction/execution and guard miss safety | release Rust unit tests | `taskset -c 1 cargo test --release quickening:: -- --nocapture` | PASS | `sem` |
| QF-OVF-01 | SmallInt overflow promotion | release rich-values test | `taskset -c 1 cargo test --release --test rich_values smallint_overflow_promotes_to_bigint_for_following_arithmetic -- --nocapture` | PASS | `sem` |
| QF-JIT-01 | adaptive JIT region execution and overflow replay | release JIT-region integration tests | `taskset -c 1 cargo test --release --test jit_region -- --nocapture` | PASS | `sem` |
| QF-JIT-02 | adaptive JIT add execution | release JIT-add integration test | `taskset -c 1 cargo test --release --test jit_add -- --nocapture` | PASS | `sem` |
| QF-CLI-01 | interpreter semantic result and no tier-up | release CLI JSON | `taskset -c 1 target/release/wustite run examples/sum_large.py --interpreter --json` | PASS | `sem` |
| QF-CLI-02 | adaptive JIT result/reuse | release CLI JSON | `taskset -c 1 target/release/wustite run examples/sum_large.py --hot-threshold 10 --repeat 2 --json` | PASS | `sem` |
| QF-BENCH-01 | release interpreter + adaptive-JIT benchmark | release CLI benchmark, CPU1 | `taskset -c 1 target/release/wustite bench examples/sum_large.py --warmup 50 --iterations 500 --hot-threshold 10` | PASS | `bench` |
| QF-CODEGEN-01 | inlining candidate removes out-of-line quick executor | release binary symbol inspection | `if nm -C target/release/wustite \| rg 'execute_quick'; then echo present; else echo 'execute_quick symbol absent'; fi` | PASS | `sem` |

## adversarialCases

| scenario id | criterion reference | adversarial class | expected behavior | verdict | artifactRefs |
|---|---|---|---|---|---|
| QF-ADV-01 | quickening runtime mismatch | wrong/unsupported operand facts | semantic path or guard miss without state corruption | PASS | `sem` |
| QF-ADV-02 | overflow semantics | i64 boundary overflow in quick/JIT path | promote to BigInt or replay safely; no wrapped result | PASS | `sem` |
| QF-ADV-03 | tier boundary | interpreter-only/high threshold | no native tier-up and correct result | PASS | `sem` |
| QF-ADV-04 | adaptive cache reuse | repeated hot execution | one compilation, subsequent native reuse, correct result | PASS | `sem` |

## artifactRefs

| id | kind | description | path |
|---|---|---|---|
| `sem` | test/CLI/symbol transcript | Focused semantic quickening, guard, overflow, JIT, CLI, and symbol evidence | `.omo/evidence/final_qa/quickening-final-fix-semantic.txt` |
| `bench` | benchmark transcript | CPU1-pinned release `sum_large.py` interpreter/adaptive-JIT benchmark | `.omo/evidence/final_qa/quickening-final-fix-benchmark.txt` |
