# Wustite quickened interpreter 성능 회귀 조사

조사일: 2026-08-06  
현재 HEAD: `f1108346a75e4dc7a6b9005c6467ecc41620de49`  
legacy 기준: `56c8400baa47d250be25a4e8637f0b0470bec49c`

원시 명령, chronological benchmark rows, PMU counts, binary hashes, assembly snippets는 `quickening-performance-raw.md`에 보존했다.

## 결론

주 병목은 quickening의 타입 사실 실패나 branch miss가 아니다. 현재 release codegen에서 hot quick path가 interpreter dispatch loop에 합쳐지지 않고, 약 300만 번 out-of-line `execute_quick`로 호출되는 것이 가장 큰 원인이다.

- `sum_large.py`의 Add/Lt는 모두 quickened된다. GenericBinary/GenericCompare 실행은 정확히 0이다.
- 현재 quickened interpreter는 legacy보다 retired instructions가 61.3%, branches가 32.6%, cycles가 36.0% 많다.
- branch miss는 실행당 약 1만 회 수준으로 양쪽 모두 매우 작다. 측정한 generic hardware cache-miss event도 차이를 설명하지 못한다.
- release assembly에서 baseline은 hot loop마다 out-of-line `execute_quick`를 호출한다. 그 함수에는 0x108-byte stack frame, 세 register bounds check, Value 복사/tag guard, Result/error 경로가 있다.
- 임시 복사본에서 `Vm::read_register` 한 곳에만 일반 `#[inline]` 힌트를 추가하면 compiler가 quick executor 전체를 dispatch loop에 합친다.
- 직접 reciprocal A/B에서 이 한 줄 후보의 elapsed time은 baseline quickened보다 약 20.5% 낮았고 legacy 대비 약 10.3% 높은 수준까지 도달했다. Adaptive JIT warm은 약 0.74 ms로 유지됐다.

따라서 1차 최소 수정안은 `Vm::read_register`에만 `#[inline]`을 추가하는 것이다. semantic bytecode, ISA, StructureMap, QuickCode 자료구조, WXIR/JIT 의미론은 바꾸지 않는다.

## 조사 방법과 통제

- OS/kernel: CachyOS Linux 7.1.6-1-cachyos
- toolchain: rustc 1.97.1, Cargo 1.97.1, LLVM 22.1.6
- build: `cargo build --release --locked`
- affinity: 모든 timing/PMU process를 `taskset -c 1`로 동일 P-core hardware thread에 고정
- workload SHA-256: `b7e095dbc0167fd51308a477640b332faf26cd2da5d36f9de839c49e88361387`
- legacy: 별도 detached worktree에서 같은 toolchain으로 build
- generic: 현재 source의 quick lookup/execute block만 제거한 임시 복사본
- quickened: 현재 source
- process가 통계적 반복 단위이며, 각 process의 내부 Interpreter median을 사용
- 기본 A/B: warmup 10, measured iterations 30, hot-threshold 10, build당 6개 fresh process
- 순서 편향 완화: `LGL | QLQ | GQG | QGQ | GLG | LQL` reciprocal ABA schedule
- PMU: 설치된 `perf` frontend가 없어 `perf_event_open` group wrapper를 임시 작성했다. user-space child만 측정하고 multiplex time을 scaling했다.

제한 사항:

- sandbox에서 `powerprofilesctl get`은 권한 오류가 났다. 사용자가 performance profile을 설정했다고 제공한 조건에 의존했다.
- sysfs governor 표시는 `powersave`였으나, 같은 CPU와 reciprocal ordering을 사용했다.
- generic build는 시간/열 변화에 민감해 IQR이 컸다. paired 비율과 PMU를 함께 해석했다.
- legacy/current 비교는 역사적 기준이다. legacy는 typed `Value::I64`/typed opcodes이고 현재는 `SmallInt`/semantic WVM이므로 pure one-factor A/B는 아니다. 현재 generic/current quickened만 pure causal A/B다.

## A. 실제 실행 경로

코드 위치:

- `src/runtime.rs:119-128`: Interpreter mode는 `hot_threshold=u64::MAX`로 VM을 생성한다.
- `src/wvm/interpreter.rs:30-44`: 매 dispatch에서 profile/JIT 처리 후 `runtime.quick_code.get(pc)`와 `execute_quick`를 시도한다.
- `src/wvm/interpreter.rs:45-55`: quick miss/guard miss일 때만 원본 semantic instruction을 fetch/match한다.
- `src/wvm/quickening.rs:32-56`: quick Add/Lt executor.
- `src/wvm/quickening.rs:58-164`: exact StructureMap facts를 사용한 one-time quick-code 생성.
- `src/wvm.rs:46-70`: `FunctionRuntime`이 runtime-local `Arc<QuickCode>`를 소유한다.

관찰 결과:

1. `ExecutionMode::Interpreter`는 실제 quick stream을 사용한다.
2. quick hit에서 원본 `Instruction`을 먼저 dispatch하지 않는다.
3. `QuickInstruction::Original(Instruction)` 같은 이중 instruction representation은 없다.
4. quick miss에서는 quick table의 bounds/Option check 뒤 semantic code의 bounds check와 enum dispatch가 이어진다.
5. `execute_quick` source는 operand 추출 match와 Add/Lt 실행 match를 각각 갖는다.
6. StructureMap/OperationSite/TypeFact는 `QuickCode::new`에서만 조회된다. hot loop 반복 조회는 없다.
7. Profile/JIT side table은 반복 조회된다. loop header마다 region lookup이 두 번 발생하고, interpreter-only threshold에서도 `plan_hot_region`까지 진행해 실패한다.
8. `sum_large.py`의 steady SmallInt path에는 clone, allocation, formatting, String 생성이 없다. register 오류 문자열과 BigInt allocation은 bounds failure/i64 overflow의 cold path다.

## B. quickening 결과와 dispatch counters

실제 16-PC bytecode/quick slot:

| PC | semantic instruction | quick slot |
|---:|---|---|
| 0 | ConstSmallInt r1=0 | None |
| 1 | Move r0←r1 | None |
| 2 | ConstSmallInt r3=1 | None |
| 3 | Move r2←r3 | None |
| 4 | ConstSmallInt r5=1 | None |
| 5 | Move r4←r5 | None |
| 6 | ConstSmallInt r7=1,000,001 | None |
| 7 | Move r6←r7 | None |
| 8 | Compare Lt r8=r2<r6, site 0 | Lt |
| 9 | Branch | None |
| 10 | Add r9=r0+r2, site 1 | Add |
| 11 | Move r0←r9 | None |
| 12 | Add r10=r2+r4, site 2 | Add |
| 13 | Move r2←r10 | None |
| 14 | Jump→8 | None |
| 15 | Return r0 | None |

Interpreter/no-JIT 한 번의 정확한 진단 counter:

| event | count |
|---|---:|
| outer dispatch | 7,000,011 |
| quick I64/SmallInt Add | 2,000,000 |
| quick I64/SmallInt Lt | 1,000,001 |
| quick guard miss | 0 |
| quick-table miss | 4,000,010 |
| generic BinaryOp | 0 |
| generic CompareOp | 0 |
| semantic Branch | 1,000,001 |
| semantic Move | 2,000,004 |
| semantic Jump | 1,000,000 |

즉 산술/비교 quickening은 정확히 작동한다. 다만 요청에서 기대한 full quick control stream은 현재 구현되어 있지 않다. Branch/Move/Jump는 sparse quick table을 miss한 뒤 원본 semantic stream에서 직접 실행된다.

진단 counter/PC dump는 임시 복사본에만 추가했으며 product source에는 남기지 않는다.

## C. 같은 환경의 legacy/generic/quickened A/B

각 값은 6개 fresh process의 내부 Interpreter median 분포다.

| 경로 | process median의 median | MAD | IQR |
|---|---:|---:|---:|
| legacy typed (L) | 34.602 ms | 0.310 ms | 0.718 ms |
| current quickened (Q) | 48.075 ms | 0.303 ms | 1.990 ms |
| current generic (G) | 140.587 ms | 8.111 ms | 17.796 ms |

reciprocal ABA의 drift-adjusted geometric ratio:

| 비교 | ratio | 해석 |
|---|---:|---|
| Q / L | 1.388 | quickened가 legacy보다 약 38.8% 느림 |
| G / L | 3.887 | generic이 legacy보다 약 288.7% 느림 |
| Q / G | 0.338 | quickened가 generic보다 약 66.2% 빠름 |

같은 schedule의 Adaptive JIT warm median:

| build | JIT warm median |
|---|---:|
| legacy | 721.173 μs |
| current quickened | 721.792 μs |
| current generic | 727.607 μs |

따라서 관찰된 interpreter 차이는 WXIR/JIT warm 경로의 회귀와 연동되지 않는다.

## D. hardware counters

`run --interpreter` 한 번을 5회 반복한 median이다. context switches와 CPU migrations는 모든 기록에서 0이었다.

| 경로 | cycles | instructions | IPC | branches | branch misses | cache misses |
|---|---:|---:|---:|---:|---:|---:|
| legacy typed | 154,461,030 | 478,389,385 | 3.097 | 102,215,760 | 8,746 | 15,451 |
| current quickened | 210,027,866 | 771,782,209 | 3.675 | 135,569,515 | 9,989 | 17,033 |
| 적용 후 product | 166,615,718 | 707,089,604 | 4.244 | 136,769,393 | 9,945 | 17,701 |
| current generic | 660,746,899 | 1,727,954,307 | 2.615 | 308,265,183 | 10,375 | 17,506 |

current quickened / legacy:

- cycles +35.97%
- instructions +61.33%
- branches +32.63%
- branch misses +1,243회
- cache misses +1,582회

적용 후 product / current quickened:

- cycles -20.67%
- instructions -8.38%
- branches +0.89%
- branch misses 사실상 동일
- IPC 3.675 → 4.244

결론적으로 branch predictor와 측정한 generic cache-miss event는 주 원인이 아니다. baseline은 더 많은 작업을 retire하고, out-of-line quick executor가 optimizer의 scheduling/fusion을 막는다. 후보가 semantic dispatch 수와 branch 수를 거의 바꾸지 않고도 cycles를 20.7% 줄인다는 점이 이를 뒷받침한다. PMU 표의 IPC는 각 counter median의 비율이며 cycles는 주파수 영향을 받으므로, retired-instruction delta와 reciprocal wall time 및 assembly를 함께 근거로 사용했다.

## E. release 생성 코드

현재 baseline:

- `Option<QuickInstruction>` element 크기: 8 bytes
- semantic `Instruction` element 크기: assembly에서 32-byte stride
- quick table access: bounds check + packed Option discriminant
- `execute_function`의 hot quick hit에서 `call wustite::wvm::quickening::execute_quick`
- `execute_quick`: 0x108-byte stack frame
- 세 register bounds check
- 두 operand Value load/copy와 combined tag guard
- Add/Lt selection
- Add fast path: checked add + overflow branch
- overflow slow path: BigInt construction/allocation
- 성공/GuardMiss/error `Result` ABI 처리

중요한 세부사항은 baseline compiler가 `read_register`/ `write_register`의 일부 동작을 `execute_quick` 내부에는 이미 펼치지만, `execute_quick` 자체를 outer dispatch loop에 합치지 않는다는 점이다.

임시 후보에서 `read_register`에 일반 `#[inline]`만 추가하면:

- `execute_quick` symbol/call이 사라진다.
- quick table lookup 뒤 operand bounds/tag/add/compare가 outer dispatcher 안에 배치된다.
- GNU `size`의 executable/read-only text aggregate 증가는 2,044 bytes, 약 0.023%다. ELF의 literal `.text` section 증가는 1,632 bytes, 약 0.028%다.

## 원인 후보: 영향도 순위

### 1. 확인됨 — quick executor의 불리한 inlining/codegen 경계

영향:

- direct A/B에서 elapsed time 약 20.5% 감소
- interleaved product PMU에서 cycles -20.7%
- IPC +18.8%

toggle proof:

- baseline: 약 49~51 ms, out-of-line `execute_quick` call 존재
- `read_register` 일반 inline hint: 약 39~42 ms, `execute_quick`가 outer loop에 fusion
- baseline binary로 되돌리면 약 49~51 ms 회귀 재현

### 2. 확인된 secondary cost group — interpreter-only profile/JIT bookkeeping

`u64::MAX` interpreter mode에서도 매 dispatch region lookup과 매 header의 profile/plan work를 수행한다. 임시로 profile 기록과 `try_execute_region`을 함께 생략하면 약 50.6 → 44.4 ms, cycles 약 -10%, instructions 약 -22%였다.

이 ablation은 profile과 tier planner를 함께 제거했으므로 각각의 기여는 아직 분리되지 않았다. 또한 profile 관찰 가능성에 영향을 줄 수 있어 이번 최소 수정 대상에서 제외한다.

### 3. 구조적 잔여 비용 — sparse quick miss + semantic control dispatch

매 실행 4,000,010회 quick-table miss가 발생한다. 이 때문에 후보도 legacy보다 instructions +47.6%, branches +33.6%다.

그러나 naïve하게 Move/Jump/Branch를 현재 `QuickInstruction` enum에 추가한 ablation은 오히려 악화됐다:

| variant | time | cycles delta | instructions delta | branches delta |
|---|---:|---:|---:|---:|
| current sparse quick | 약 50.6 ms | 기준 | 기준 | 기준 |
| naïve full-control enum | 약 64.6 ms | +26% | +13% | +16% |

layout 측정:

- current `QuickInstruction` / `Option<QuickInstruction>`: 8 / 8 bytes
- naïve control 확장: 24 / 24 bytes

따라서 full quick stream 자체가 잘못된 것이 아니라, 현재 enum을 그대로 비대화하는 구현이 code density와 dispatch codegen을 악화한다. compact encoding/fused executor는 별도 계측 과제로 남겨야 하며 이번에 추측으로 리팩터링하지 않는다.

### 4. 기각 — exact fact/guard 실패

quick guard miss, GenericBinary, GenericCompare 모두 0이다. StructureMap 사실은 원인이 아니다.

### 5. 기각 — branch miss와 측정한 generic cache-miss event

branch/cache miss 절대량과 증분이 너무 작고, one-line 후보에서 둘이 그대로인데 cycles가 크게 감소한다. 이 측정만으로 L1I/ITLB/frontend stall 전체를 배제하지는 않는다.

## 최소 수정 후보와 직접 A/B

후보:

```rust
#[inline]
pub(super) fn read_register(frame: &Frame, register: Register) -> Result<Value, String> {
    // 기존 body 그대로
}
```

legacy(L), current quickened(Q), one-line 후보(R)의 reciprocal schedule:

`LQL | RLR | QRQ | RQR | LRL | QLQ`

| 경로 | process median의 median | MAD | IQR |
|---|---:|---:|---:|
| L | 37.340 ms | 0.649 ms | 2.208 ms |
| Q | 51.483 ms | 1.029 ms | 2.058 ms |
| R | 41.922 ms | 1.383 ms | 2.774 ms |

paired drift-adjusted ratio:

- Q/L = 1.380
- R/Q = 0.795 — 후보의 elapsed time이 baseline quickened보다 약 20.5% 낮음
- R/L = 1.103 — 후보가 legacy보다 약 10.3% 느림

이 schedule의 JIT warm median은 Q=754.815 μs, R=745.062 μs였다. 후보에서 JIT warm 회귀가 없다.

## 안전성 평가

낮은 위험:

- 일반 inline hint 한 줄이며 `unsafe`, unchecked indexing, transmute가 없다.
- error/overflow semantics와 register bounds check를 그대로 유지한다.
- semantic bytecode는 immutable이다.
- Python frontend와 semantic opcode는 바뀌지 않는다.
- StructureMap은 정적 사실만 보존한다.
- QuickCode는 FunctionRuntime-local 실행 계층으로 유지된다.
- WXIR/JIT 코드는 변경하지 않는다.
- binary text 증가는 0.023% 수준이다.

남은 위험:

- inline은 optimizer 결정이므로 rustc/LLVM version에 민감하다.
- timing assertion을 unit test로 넣으면 CI에서 flaky하다. 의미론 회귀는 기존 static-quickening, numeric-overflow, JIT tests로 검증하고 성능은 이 문서의 pinned-process benchmark/PMU 절차로 검증하는 편이 안전하다.

## 권고

1. 1차로 `Vm::read_register`에 일반 `#[inline]` 한 줄만 적용한다.
2. `cargo fmt --check`, `cargo clippy --all-targets --all-features -- -D warnings`, `cargo test --all-targets`를 실행한다.
3. release binary를 다시 만들고 동일 reciprocal schedule과 PMU를 재실행한다.
4. interpreter-only tier bookkeeping은 profile 관찰 계약을 먼저 결정한 뒤 별도 one-factor 조사한다.
5. full quick stream은 8-byte compact representation을 유지할 설계와 executor fusion을 함께 측정하기 전에는 구현하지 않는다.

## 적용 후 검증

적용:

- `src/wvm.rs:224`의 `Vm::read_register`에 일반 `#[inline]` 한 줄 추가
- 공개 API, ISA, semantic bytecode, QuickCode layout, StructureMap, WXIR/JIT code 변경 없음
- 별도 timing assertion은 추가하지 않았다. compiler inlining을 unit test로 고정하면 toolchain/platform 의존적인 flaky test가 되므로, 기존 semantic quickening/overflow tests와 이 고정-hardware benchmark를 사용한다.

정적/기능 검증:

| command | 결과 |
|---|---|
| `cargo fmt --all -- --check` | PASS |
| `cargo clippy --all-targets --all-features -- -D warnings` | PASS |
| `cargo test --all-targets` | PASS, 122 tests |
| `cargo test --release --all-targets` | PASS, 122 tests |
| `cargo build --release --locked` | PASS |
| release `nm -C` | `execute_quick` symbol/call 없음 |

실제 product binary와 legacy의 reciprocal schedule `LFL|FLF|LFL|FLF`:

| 경로 | process median의 median | MAD | IQR |
|---|---:|---:|---:|
| legacy | 35.000 ms | 0.520 ms | 1.041 ms |
| fixed product | 38.665 ms | 0.060 ms | 0.193 ms |

- 네 reciprocal ABA block의 drift-adjusted fixed/legacy ratio: 1.110
- 즉 legacy 대비 elapsed time 약 11.0% 차이로 성공 기준 안에 들어왔다.

saved pre-fix binary와 fixed product의 reciprocal schedule에서는 pre-fix run 하나가 96.345 ms/JIT 1.531 ms로 동시에 튄 system outlier가 있었다. 제거하지 않고 보존했으며, 네 paired block의 log-median fixed/pre-fix ratio는 0.794였다. 즉 outlier를 포함한 robust paired estimator에서도 elapsed time이 약 20.6% 감소했다.

요청 예시와 동일한 `--warmup 50 --iterations 500 --hot-threshold 10` 단일 장시간 run:

| 경로 | Interpreter median | Adaptive JIT warm |
|---|---:|---:|
| pre-fix quickened | 48.068 ms | 724.085 μs |
| fixed product | 39.508 ms | 726.299 μs |
| legacy typed | 36.473 ms | 721.754 μs |

- fixed/pre-fix elapsed time: -17.8%
- fixed/legacy elapsed time: +8.3%
- fixed JIT warm/pre-fix JIT warm: +0.3%, 실질적으로 유지

최종 interleaved product PMU 5회 median은 cycles=166,615,718, instructions=707,089,604, branches=136,769,393, branch-misses=9,945, cache-misses=17,701, IPC=4.244였다. 모든 run의 context switches와 CPU migrations는 0이었다.
