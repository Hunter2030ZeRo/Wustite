# Python function identity validation

## Scope shadowing

- Given: `SHADOWING_SOURCE` defines a top-level `helper`, a parameter named
  `helper`, a parameter named after its current function, and a local named
  `helper`.
- When: `initialized_parameters_and_locals_shadow_top_level_function_names`
  compiles all three functions and executes them with the WVM interpreter.
- Then: the binary observable is `Value::SmallInt(17)`,
  `Value::SmallInt(23)`, and `Value::SmallInt(7)`; the test passed.
- Invocation: `cargo test --test python_function_identity`.

## Helper identity and calls

- Given: `HELPER_IDENTITY_SOURCE` references top-level `increment` twice.
- When: `repeated_top_level_helper_references_share_identity_and_remain_callable`
  executes `first == second and first(41) == 42 and second(99) == 100`.
- Then: the binary observables are two identical helper `ExecutableId` values
  and `Value::Bool(true)`; this proves object equality and both calls execute.
- Invocation: `cargo test --test python_function_identity`.

## Python lexical locals and cycle safety

- Given: `FUTURE_LOCAL_SOURCE` uses `helper` before assigning that simple name
  later in the same function.
- When: `later_local_assignment_blocks_top_level_function_resolution` compiles
  `main`.
- Then: the binary observable is the frontend error
  `name \`helper\` is not initialized`; the top-level helper is not captured.
- Given: `CYCLE_SOURCE` has two unresolved top-level functions that reference
  one another.
- When: `function_reference_cycles_remain_rejected` compiles `main`.
- Then: the binary observable is an error containing
  `recursive function reference cycle`.
- Invocation: `cargo test --test python_function_identity`.

## Regression and quality gates

- Red proof: before the frontend change, the targeted invocation exited 101;
  scope shadowing returned `Object(...)` instead of `SmallInt(17)`, and helper
  identity returned `Bool(false)` instead of `Bool(true)`.
- Green invocation: `cargo test --test python_frontend --test python_rich_values --test python_function_identity`.
  Observable: 17 passed, 0 failed (3 frontend, 10 rich values, 4 identity).
- Formatting invocation: `rustfmt --edition 2024 --check src/frontend/python/mod.rs src/frontend/python/expression.rs src/frontend/python/statements.rs tests/python_function_identity.rs`.
  Observable: exit 0.
- Lint invocation: `cargo clippy --test python_function_identity -- -D warnings`.
  Observable: exit 0.
- Diff hygiene invocation: `git diff --check -- src/frontend/python/mod.rs src/frontend/python/expression.rs src/frontend/python/statements.rs tests/python_function_identity.rs`.
  Observable: exit 0.

## Maintainability gate

- Given: expression lowering exceeded the project 250 pure-LOC cap after the
  lexical-scope plumbing.
- When: literal parsing and binary/comparison operator parsing moved into
  `src/frontend/python/expression/literals.rs`.
- Then: the measured pure-LOC counts are 228 for `expression.rs` and 72 for
  `expression/literals.rs`; both are below the cap with behavior preserved.
- Invocation: `cargo test --test python_frontend --test python_rich_values --test python_function_identity && cargo clippy --test python_function_identity -- -D warnings`.
  Observable: 17 tests passed and clippy exited 0.
