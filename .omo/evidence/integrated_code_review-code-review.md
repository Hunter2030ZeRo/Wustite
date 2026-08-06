# Code Quality Review — integrated_code_review

## Scope and evidence

Reviewed the full current working-tree diff for the rich-value ISA and execution ABI work, including the object heap, interpreter, JIT replay/admission, verifier, frontend lowering, CLI surface, and added tests.

Checks run on the final observed snapshot:

- `cargo test --all-targets` — PASS (all 67 tests)
- `cargo clippy --all-targets --all-features -- -D warnings` — PASS
- `git diff --check` — PASS
- `cargo +nightly miri test --test jit_region --test rich_values` — could not complete: Cranelift JIT calls `mprotect(PROT_EXEC)`, which Miri does not support. This is a tooling limitation rather than evidence of UB; no strict-provenance Miri result is available for the generated-code path.

The prior evidence artifacts were inspected but treated as untrusted. Some record earlier red-state compilation failures, so they are not used to support the final green result.

## Required skill-perspective check

Ran the `remove-ai-slops` and `programming` skill perspectives, including the Rust and Rust-UB guidance because the patch retains an unsafe native JIT entry call.

- `remove-ai-slops`: no deletion-only, tautological, implementation-mirroring, or prompt-prose test was found. The new behavior tests exercise observable interpreter/JIT/ABI outcomes. The extracted modules have clear responsibilities; no needless parsing/normalization was found in production paths beyond the CLI input boundary.
- `programming`: the diff does violate the public-API compatibility perspective below. The unsafe-entry Miri proof required by the skill is unavailable because generated executable memory cannot run under Miri; this is a verification gap, not classified as a code defect here.

## Findings

### CRITICAL

None.

### HIGH

1. **Breaking public API rename with no compatibility layer.**
   - `src/value.rs:5-10`
   - `src/runtime/value.rs:8-13`
   - `src/structure_map.rs:5-10`

   `Value::I64`, `RuntimeValue::I64`, and `SlotType::I64` were removed and replaced with `SmallInt`. These are public types/variants, so existing embedding clients no longer compile. The source comment preserves only legacy *opcodes* (`ConstI64`, `AddI64`, `LtI64`); it does not preserve the corresponding public value/type API. Existing repository tests were mechanically migrated to the new names, so the suite cannot detect the consumer-facing break. Preserve deprecated aliases/variants (or explicitly make this a documented breaking release with a migration plan) before approval.

### MEDIUM

None.

### LOW

None.

## Focused review conclusions

- Object refs use heap identity plus a generation and reject cross-heap/stale refs. The final tests cover both isolation and slot reuse.
- SmallInt overflow exits native code before destination mutation and replays in the interpreter; the direct replay and subsequent BigInt arithmetic coverage is relevant.
- Cross-function function values are covered; no evidence of a semantic call/replay defect was found. Recursive/cyclic frontend support remains intentionally constrained by the supported Python subset.
- Rich operations intentionally fail JIT admission and continue interpreted. The latest object-entry JIT test confirms this remains recoverable and does not disable the cached native region.
- The verifier covers the new instruction operands, constants, ABI register duplication/names, operation sites, and loop metadata. No artifact-backed verifier hole was found.

## Recommendation

- `codeQualityStatus`: BLOCK
- `recommendation`: REQUEST_CHANGES
- `blockers`:
  - Restore or explicitly version/document the removed public `I64` API surface (`Value`, `RuntimeValue`, `SlotType`) with a consumer migration path.
