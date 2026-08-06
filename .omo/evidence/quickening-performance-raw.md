# Quickening performance raw evidence

이 파일은 `quickening-performance-investigation.md`의 재현 가능한 원시 자료다. 모든 숫자는 2026-08-06에 CPU 1로 고정해 수집했다. 임시 source copy와 binary는 cleanup에서 제거하므로 hash, 명령, chronological output을 여기에 보존한다.

## Provenance

```text
rustc 1.97.1 (8bab26f4f 2026-04-23)
cargo 1.97.1 (b49fab452 2026-03-11)
LLVM version: 22.1.6
current HEAD: f1108346a75e4dc7a6b9005c6467ecc41620de49
legacy: 56c8400baa47d250be25a4e8637f0b0470bec49c
```

SHA-256:

```text
b7e095dbc0167fd51308a477640b332faf26cd2da5d36f9de839c49e88361387  examples/sum_large.py
f71c22ad878999ad606a58b1c8a4199f80504071647a7d6b61320b1c3c171b5a  fixed target/release/wustite
86d552ba7117df029dc1ad696ce6ca7d1a12bc8c8c09a9eae327f406c96f7497  legacy wustite
78bdc29245fabb43df8587dbc0fe45e7c94d1df6581354ae4826b476ca6f3be6  pre-fix quickened wustite
5c61fcf0aca358ca24a9febf652f25f44ea1a23850e746735aacfd159297d5be  generic-current wustite
6de5045e736b5abc639a3ba8b06eba4e6ceb08bbc18c6fea8911ab199e0c88a8  perfwrap
```

Build commands:

```bash
cargo build --release --locked
git worktree add --detach /tmp/wustite-legacy-56c8400 56c8400baa47d250be25a4e8637f0b0470bec49c
(cd /tmp/wustite-legacy-56c8400 && cargo build --release --locked)
cc -O2 -Wall -Wextra -o /tmp/wustite-quickening-perf/perfwrap .omo/evidence/perfwrap.c
```

The generic-current diagnostic removed only `src/wvm/interpreter.rs:39-44`, the `runtime.quick_code.get(frame.pc)` / `execute_quick` block. It retained the same semantic bytecode, profile, JIT bookkeeping, release flags, and lockfile.

## Quick-code PC dump and counters

Command:

```bash
WUSTITE_QUICK_DIAGNOSTICS=1 taskset -c 1 DIAGNOSTIC_BIN run examples/sum_large.py --interpreter
```

One-time PC dump:

```text
pc=0  semantic=ConstSmallInt r1=0                       quick=None
pc=1  semantic=Move r0<-r1                             quick=None
pc=2  semantic=ConstSmallInt r3=1                       quick=None
pc=3  semantic=Move r2<-r3                             quick=None
pc=4  semantic=ConstSmallInt r5=1                       quick=None
pc=5  semantic=Move r4<-r5                             quick=None
pc=6  semantic=ConstSmallInt r7=1000001                 quick=None
pc=7  semantic=Move r6<-r7                             quick=None
pc=8  semantic=CompareOp Lt r8=r2<r6 site=0             quick=Some(Lt r8,r2,r6)
pc=9  semantic=Branch cond=r8 yes=10 no=15              quick=None
pc=10 semantic=BinaryOp Add r9=r0+r2 site=1             quick=Some(Add r9,r0,r2)
pc=11 semantic=Move r0<-r9                              quick=None
pc=12 semantic=BinaryOp Add r10=r2+r4 site=2            quick=Some(Add r10,r2,r4)
pc=13 semantic=Move r2<-r10                             quick=None
pc=14 semantic=Jump target=8                            quick=None
pc=15 semantic=Return r0                                quick=None
```

Exact output counters:

```text
quick-counts add=2000000 lt=1000001 guard_miss=0 generic_binary=0 generic_compare=0 branch=1000001 move=2000004 jump=1000000
500000500000
```

Derived from the same counter run:

```text
outer_dispatch=7000011
quick_table_miss=4000010
```

Diagnostic counter/dump code existed only in the temporary copy and was never applied to product source.

## Primary legacy/generic/quickened benchmark

Command for every process:

```bash
taskset -c 1 BIN bench /home/entity27th/Wustite/examples/sum_large.py \
  --function main --warmup 10 --iterations 30 --hot-threshold 10
```

Schedule: `LGL | QLQ | GQG | QGQ | GLG | LQL`.

```text
run build interpreter_median_ms jit_warm_median_us
01  L     35.241                705.857
02  G    129.726                730.283
03  L     34.061                721.080
04  Q     50.214                721.940
05  L     34.633                717.578
06  Q     47.851                727.829
07  G    133.043                722.841
08  Q     49.841                724.029
09  G    148.131                734.096
10  Q     47.998                720.479
11  G    148.636                724.930
12  Q     47.692                721.644
13  G    148.759                746.377
14  L     34.571                721.266
15  G    130.840                722.381
16  L     36.205                723.253
17  Q     48.151                721.637
18  L     34.523                722.509
```

No run was discarded. Per-build medians are L=34.602 ms, Q=48.075 ms, G=140.587 ms.

For each ABA block the endpoint interpolation is geometric:

```text
z = ln(B) - (ln(A_before) + ln(A_after))/2
```

The two reciprocal estimates per comparison give descriptive centers Q/L=1.388409, G/L=3.887202, Q/G=0.338056. They are descriptive statistics, not confidence intervals.

## One-line candidate three-way benchmark

`R` is a build whose only source difference from pre-fix current is ordinary `#[inline]` on `Vm::read_register`.

Schedule: `LQL | RLR | QRQ | RQR | LRL | QLQ`.

```text
run build interpreter_median_ms jit_warm_median_us
01  L     35.641                742.215
02  Q     52.548                776.281
03  L     38.127                788.085
04  R     43.489                762.194
05  L     37.849                752.375
06  R     43.120                753.165
07  Q     52.386                760.476
08  R     41.617                737.438
09  Q     54.994                755.122
10  R     40.346                750.142
11  Q     50.580                738.380
12  R     42.226                739.981
13  L     35.358                727.675
14  R     38.522                737.923
15  L     37.164                726.956
16  Q     49.762                754.507
17  L     37.515                731.958
18  Q     50.490                729.846
```

Medians/MAD/Tukey-IQR:

```text
L 37.3395 / 0.6485 / 2.208
Q 51.4830 / 1.0290 / 2.058
R 41.9215 / 1.3830 / 2.774
```

Descriptive reciprocal centers: Q/L=1.380085, R/Q=0.795442, R/L=1.102654.

## Final product reciprocal benchmarks

Legacy versus fixed product, schedule `LFL | FLF | LFL | FLF`:

```text
run build interpreter_median_ms jit_warm_median_us
01  L     34.165                722.136
02  F     38.669                724.222
03  L     35.835                721.728
04  F     38.661                722.833
05  L     35.501                720.455
06  F     38.634                722.050
07  L     34.627                754.748
08  F     38.988                733.393
09  L     35.372                722.887
10  F     38.827                732.366
11  L     34.460                729.158
12  F     38.575                725.401
```

Medians: L=35.000 ms, F=38.665 ms. Four paired F/L ratios sorted are 1.088631, 1.105143, 1.114022, 1.123064; log-median center=1.109574.

Pre-fix versus fixed product, schedule `QFQ | FQF | QFQ | FQF`:

```text
run build interpreter_median_ms jit_warm_median_us
01  Q     48.667                 739.904
02  F     38.676                 756.043
03  Q     50.475                 754.850
04  F     39.231                 721.419
05  Q     47.845                 719.697
06  F     38.894                 722.449
07  Q     48.025                 723.649
08  F     39.083                 726.398
09  Q     96.345                1531.000
10  F     39.366                 727.470
11  Q     48.501                 727.751
12  F     39.032                 738.553
```

Run 09 is retained: interpreter and JIT both doubled, indicating a system-wide transient rather than a quickening-only event. The four paired F/Q ratios sorted are 0.574566, 0.780344, 0.808203, 0.816431; robust log-median center=0.794151.

## Long benchmark output

Command:

```bash
taskset -c 1 BIN bench examples/sum_large.py \
  --function main --warmup 50 --iterations 500 --hot-threshold 10
```

```text
build             Interpreter median  P95       Adaptive JIT warm median
pre-fix quickened 48.068 ms           49.316 ms 724.085 us
fixed product     39.508 ms           42.278 ms 726.299 us
legacy typed      36.473 ms           37.579 ms 721.754 us
```

All three exited 0. Full fixed output additionally reported cold JIT=1.343 ms, compilation attempts=1, compiled regions=1, native executions=1.

Independent QA repeated the fixed command and captured the full transcript at `.omo/evidence/final_qa/quickening-final-fix-benchmark.txt`: fixed interpreter=38.723 ms, JIT warm=724.335 us.

## Interleaved PMU raw output

Driver source: `perfwrap.c`.

Command for each row:

```bash
./perfwrap taskset -c 1 BIN run examples/sum_large.py --interpreter
```

Every run printed result `500000500000`, exited 0, and reported context_switches=0 and cpu_migrations=0. Counts below are already scaled by `time_enabled/time_running`.

```text
rep build time_enabled time_running cycles instructions branches branch_misses cache_misses
1 L 34077558 33316652 151971348 479502264 102453506  8746 15451
1 Q 50124789 49611135 209570364 771782209 135569515  9858 16824
1 G 152436944 151900584 662261999 1727954307 308265183 10131 17506
1 F 41648992 41100191 167010623 708203989 136984905  9936 17711
2 L 36676851 35845065 154461030 479674503 102490314  8878 15526
2 Q 53192238 52523606 219921825 773598015 135888509  9989 17033
2 G 153363519 152927564 660746899 1726893009 308081433 10776 17790
2 F 41620916 41137816 166256980 707079663 136767464  9883 17120
3 L 39004807 38222629 158167833 478389385 102215760  8758 15869
3 Q 53042300 52522099 210027866 771438157 135509028 10017 16588
3 G 159740792 159137599 662279580 1728400088 308344667 10215 17483
3 F 40881080 40406020 165401162 707089604 136769393  9945 16901
4 L 36204265 35668519 151753695 475838283 101670683  8675 15267
4 Q 52711021 51982587 219238108 774577668 136060589  9952 18300
4 G 139744671 139220635 587367364 1728354194 308336426 10375 16798
4 F 43163057 42668966 166615718 706964825 136745237 10079 17731
5 L 37456200 36953272 156779222 475176078 101529158  8733 14967
5 Q 51266827 50749451 208058756 771661246 135548315 10253 17464
5 G 136550458 136550458 575963905 1722208956 307255019 13882 19717
5 F 42211430 41646029 166734960 708360250 137015126 10006 17701
```

Per-counter medians:

```text
build cycles instructions branches branch_misses cache_misses IPC_from_medians
L 154461030  478389385 102215760  8746 15451 3.097
Q 210027866  771782209 135569515  9989 17033 3.675
G 660746899 1727954307 308265183 10375 17506 2.615
F 166615718  707089604 136769393  9945 17701 4.244
```

The generic cycles are visibly bimodal while its retired instructions remain stable. This is why the investigation gives greatest weight to retired instructions, reciprocal wall-time measurements, and assembly rather than unpaired cycle counts alone.

## Assembly and layout evidence

Baseline symbols:

```text
000000000030afc0 t <wustite::wvm::Vm>::execute_function
0000000000362490 t wustite::wvm::quickening::execute_quick
000000000030aed0 t <wustite::wvm::Vm>::read_register
000000000030af40 t <wustite::wvm::Vm>::write_register
```

Baseline quick hit call site:

```asm
30b9e8: mov    0x78(%rsp),%rax
30b9ed: cmp    0x18(%rcx),%rax      # QuickCode bounds
30b9f7: mov    (%rcx,%rax,8),%rsi  # 8-byte Option<QuickInstruction>
30b9fb: cmp    $0x2,%si             # None discriminant
30ba13: call   362490 <wustite::wvm::quickening::execute_quick>
```

Baseline quick executor prologue/checks:

```asm
362490: push   %rbp
362491: push   %r15
362493: push   %r14
362495: push   %r13
362497: push   %r12
362499: push   %rbx
36249a: sub    $0x108,%rsp
3624b8: cmp    %r13,%r8             # dst bounds
3624dc: cmp    %rax,%r8             # lhs bounds
362518: cmp    %r9,%r8              # rhs bounds
36253f: jne    362614               # combined tag guard miss
362624: add    %r14,%rsi
362627: jo     3626f3               # BigInt overflow slow path
```

Commands:

```bash
nm -C BIN | rg 'execute_quick|wvm::Vm>::(read_register|write_register)|execute_function'
objdump -Cd --disassemble='<wustite::wvm::Vm>::execute_function' BIN
objdump -Cd --disassemble='wustite::wvm::quickening::execute_quick' BIN
```

Fixed product symbols:

```text
000000000036a8a0 t <wustite::wvm::Vm>::read_register
000000000036a910 t <wustite::wvm::Vm>::write_register
000000000036a990 t <wustite::wvm::Vm>::execute_function
# no execute_quick symbol
```

Fixed product quick dispatch is fused into `execute_function`:

```asm
36b1f0: cmp    0x18(%rcx),%rax
36b1fa: mov    (%rcx,%rax,8),%rsi
36b1fe: cmp    $0x2,%si
36b202: jne    36b2a4               # direct fused quick path
36b2a7: shr    $0x10,%rdx           # unpack dst
36b2bc: cmp    %rcx,%r14            # dst bounds
36b2dc: cmp    %rcx,%rdx            # lhs bounds
36b30b: cmp    %rcx,%r8             # rhs bounds
36b31e: or     (%r12,%rcx,8),%r9b   # combined tag guard
36b32d: test   $0x1,%sil            # Add/Lt selection
```

There is no call to `execute_quick` in that path.

Temporary release layout tests:

```text
current: QuickInstruction=8 Option<QuickInstruction>=8
naive Move/Jump/Branch extension: QuickInstruction=24 Option<QuickInstruction>=24
```

GNU `size` output:

```text
text     data   bss   dec      binary
8805972 155424 3760  8965156  pre-fix
8808016 155592 1592  8965200  ordinary read-inline candidate
```

The aggregate text delta is +2,044 bytes (+0.0232%). `llvm-objdump -h`/ELF section inspection gives literal `.text` delta +1,632 bytes (+0.0282%).

## Validation transcripts

- `.omo/evidence/final_qa/quickening-final-fix-manual-qa.md`
- `.omo/evidence/final_qa/quickening-final-fix-semantic.txt`
- `.omo/evidence/final_qa/quickening-final-fix-benchmark.txt`
- `.omo/evidence/quickening-performance-investigation-code-review.md` records the initial review blocker that prompted preservation of this raw evidence.
