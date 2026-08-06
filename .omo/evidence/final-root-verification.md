# Final root verification: WVM static quickening

Verified on 2026-08-06 against the final source after exhaustive opcode matching and test-module extraction.

## Automated checks

- `cargo test --all-targets`: PASS, 122 passed and 0 failed.
- `cargo clippy --all-targets --all-features -- -D warnings`: PASS.
- `cargo fmt --all -- --check`: PASS.
- `git diff --check`: PASS.
- `MIRIFLAGS=-Zmiri-tree-borrows cargo +nightly miri test --lib quickening`: PASS, 4 passed.
- `MIRIFLAGS=-Zmiri-tree-borrows cargo +nightly miri test --test static_quickening`: PASS, 2 passed; the native-JIT fixture is intentionally ignored under Miri.

## Scope and invariants

- Added production pure LOC: 187, below the plan cap of 250.
- Current touched-file pure LOC: `src/wvm.rs` 215, `arithmetic.rs` 249, `callables.rs` 88, `interpreter.rs` 221, and `quickening.rs` 169; each is at most 250.
- Protected Cargo, semantic bytecode, executable, StructureMap, verifier, frontend fact lowering, JIT runtime, and WXIR files compare byte-for-byte with the scoped baseline.
- `QuickCode` contains only optional private quick instructions keyed one-to-one by semantic PC; it stores no `Value` or `ObjectRef`.
- All public `Instruction`, `BinaryOperator`, and `CompareOperator` variants are exhaustively classified by the constructor.

## Independent verification

- Code review: `static-quickening-code-review.md` — APPROVED, no findings.
- Manual QA: `final_qa/static_quickening-manual-qa.md` — PASS across exact, fallback, overflow, JIT replay, clone/runtime, and recursion scenarios.
- Final gate: `static-quickening-gate-review.md` — APPROVED, no blockers.

No commit was created.
