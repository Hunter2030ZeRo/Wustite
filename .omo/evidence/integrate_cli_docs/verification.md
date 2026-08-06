# CLI/docs integration verification

## Owned-file quality gate

- Scenario: all Rust files owned by this task are formatted, contain no production `unwrap`/`expect`/`panic` calls, and remain below the 250 pure-LOC ceiling.
- Invocation: `rustfmt --edition 2024 --check src/cli.rs src/cli/arguments.rs src/cli/benchmark.rs src/cli/inspection.rs src/cli/report.rs src/cli/value_names.rs && git diff --check -- src/cli.rs src/cli README.md && ! rg -n '\b(unwrap|expect|panic)!?\s*\(' src/cli.rs src/cli && for file in src/cli.rs src/cli/*.rs; do count=$(awk '!/^[[:space:]]*$/ && !/^[[:space:]]*\/\//' "$file" | wc -l); test "$count" -le 250 || exit 1; printf '%s %s pure_LOC\n' "$file" "$count"; done`
- Binary observable: exit code `0`.
- Captured output:

```text
src/cli.rs 149 pure_LOC
src/cli/arguments.rs 88 pure_LOC
src/cli/benchmark.rs 172 pure_LOC
src/cli/inspection.rs 109 pure_LOC
src/cli/report.rs 190 pure_LOC
src/cli/value_names.rs 21 pure_LOC
```

## Integrated compile/test gate

- Scenario: compile the CLI binary against the concurrent runtime/frontend implementation.
- Invocation: `cargo check --bin wustite`
- Binary observable: exit code `101`; integration was not runnable because concurrent, out-of-scope WVM modules had not landed.
- Captured blocker: compiler errors `E0583` for missing `src/wvm/arithmetic.rs`, `interpreter.rs`, `jit_runtime.rs`, `objects.rs`, and `registers.rs`, followed by stale out-of-scope HIR/JIT references. The root integrator was notified and will run the integrated CLI tests after those files converge.

## Implemented scenarios

- `run`: compiles first, parses each `--arg` from executable parameter metadata, allocates String/BigInt objects in the executing runtime, and snapshots result object kind plus `heap_id`/`slot`/`generation` before runtime drop.
- `bench`: independently parses and allocates arguments for interpreter and adaptive runtimes, preventing cross-runtime object handles.
- `inspect`: emits `small_int`, `float`, `bool`, object-kind names, and `any` from the expanded `SlotType` model.
- Documentation: records the scalar/object model, ObjectRef lifetime rule, supported subset, BigInt overflow/JIT replay, and interpreter-only rich values.
