# Wustite 
<img width="2172" height="724" alt="ChatGPT Image 2026년 7월 26일 오후 10_38_39" src="https://github.com/user-attachments/assets/3f824541-9591-494e-a7fd-b8e854c571f2" />

**Wustite** is an experimental end-to-end runtime prototype for Python under active development, based on register-based VM and pre-analysis augmented JIT compilation. 

> [!NOTE]
> Wustite is currently an early prototype. This README describes both
> the implemented architecture and the intended production design.

## Why does Wustite matter?

CPython is known to consume up to 75x more energy and run up to 76x slower 
than native C code [[1](#1)]. Alternative runtimes have tried to close 
this gap via JIT compilation — however, either their development 
has largely stalled, or remains active but suffers from poor 
C extension compatibility (NumPy, pandas, etc.) due to its non-CPython 
memory model.

Wustite takes a different approach. Instead of relying on runtime profiling 
to warm up a JIT, Wustite statically builds a **structure map** via bytecode lowering, which is ahead-of-time-generated 
metadata passed into the JIT compiler that contains control flow, inferred type, estimated hot loops, etc. 
This lets the runtime skip the warm-up phase entirely and eliminates interpretation 
overhead at execution. This is paired with **Wustite Virtual Machine(WVM)**, 
a register-based bytecode virtual machine that is 
CPython-compatible — so Wustite aims to deliver both the performance of 
ahead-of-time compilation and the efficiency of a runtime that doesn't 
sacrifice compatibility to get there.

WVM compiles down to **Wustite eXtensive Intermediate Representation(WXIR)**, which is 
Wustite's specific IR designed for backend-independent SSA optimization. Due to this, 
Wustite can use various backends including Cranelift and LLVM.

## Current prototype

The current prototype already implements the core vertical pipeline:

Python source → HIR → WVM → Structure Map and profiling
→ WXIR → Cranelift → native region execution → WVM side exit

The supported Python language surface is still intentionally restricted
while the runtime architecture is being stabilized.

## CLI prototype

The current CLI executes one named function from the supported Python subset.
Values are passed with a repeatable `--arg` option and parsed from each
parameter's annotation after the function is compiled. The CLI accepts `int`
(`small_int`), `float`, `bool` (`true` or `false`), `str`, and arbitrary-size
`BigInt` arguments. Tuple, list, dict, function, and `Any` parameters must be
constructed through the Runtime API. It is not a drop-in replacement for the
CPython command line.

```text
cargo run -- run examples/sum.py
cargo run -- run examples/sum.py --repeat 2 --hot-threshold 10 --trace-jit
cargo run -- run examples/sum.py --interpreter
cargo run -- run examples/add.py --function add --arg 20 --arg 22
cargo run -- inspect examples/sum.py
```

Use `--function NAME` to select a function and `--json` for structured output.

## Runtime value model

The public `RuntimeValue` boundary distinguishes three immediate scalar values
from heap-backed objects:

| Runtime value | WVM type | Representation |
| --- | --- | --- |
| `SmallInt(i64)` | `small_int` | Signed 64-bit immediate integer |
| `Float(f64)` | `float` | IEEE-754 double-precision immediate |
| `Bool(bool)` | `bool` | Boolean immediate |
| `Object(ObjectRef)` | object kind | Runtime-owned heap handle |

Object kinds currently include string, tuple, arbitrary-precision `BigInt`,
list, dict, and closureless function objects. An `ObjectRef` contains a runtime
heap ID, slot, and generation. It is meaningful only while its owning `Runtime`
is alive, and handles from one runtime must never be passed to another runtime.
Use `Runtime::object` or `Runtime::object_kind` while the owner is alive; CLI
output snapshots the object kind and handle fields before the runtime is
dropped.

Small integers promote to a heap `BigInt` on overflow instead of wrapping.
That promotion and other rich-value operations currently execute in the
interpreter. Adaptive JIT regions side-exit to WVM so the operation can be
replayed with Python semantics; rich values are not yet native-specialized.
The public scalar variants formerly named `I64` are intentionally renamed to
`SmallInt`; legacy `ConstI64`, `AddI64`, and `LtI64` bytecode opcodes remain
available while hand-authored executables migrate to the semantic ISA.

## Supported Python subset

The prototype supports named, top-level closureless functions with typed positional
parameters; integer, float, Boolean, string, tuple, list, and dict literals;
local variables; selected arithmetic, comparison, Boolean, indexing, `len`,
calls, returns, and structured control flow used by the examples and tests.
Imports, modules, globals, classes, exceptions, generators, comprehensions,
decorators, default/keyword/variadic parameters, and general CPython extension
compatibility are not implemented yet. Some accepted rich-value operations are
interpreter-only as described above.


## References 

<a id="1">[1]</a> 
Rui Pereira, Marco Couto, Francisco Ribeiro, Rui Rua, Jácome Cunha, João Paulo Fernandes, João Saraiva,
Ranking programming languages by energy efficiency,
Science of Computer Programming,
Volume 205,
2021,
102609,
ISSN 0167-6423,
https://doi.org/10.1016/j.scico.2021.102609.
(https://www.sciencedirect.com/science/article/pii/S0167642321000022)
Abstract: This paper compares a large set of programming languages regarding their efficiency, including from an energetic point-of-view. Indeed, we seek to establish and analyze different rankings for programming languages based on their energy efficiency. The goal of being able to rank programming languages based on their energy efficiency is both recent, and certainly deserves further studies. We have taken rigorous and strict solutions to 10 well defined programming problems, expressed in (up to) 27 programming languages, from the well known Computer Language Benchmark Game repository. This repository aims to compare programming languages based on a strict set of implementation rules and configurations for each benchmarking problem. We have also built a framework to automatically, and systematically, run, measure and compare the energy, time, and memory efficiency of such solutions. Ultimately, it is based on such comparisons that we propose a series of efficiency rankings, based on single and multiple criteria. Our results show interesting findings, such as how slower/faster languages can consume less/more energy, and how memory usage influences energy consumption. We also present a simple way to use our results to provide software engineers and practitioners support in deciding which language to use when energy efficiency is a concern. In addition, we further validate our results and rankings against implementations from a chrestomathy program repository, Rosetta Code., by reproducing our methodology and benchmarking system. This allows us to understand how the results and conclusions from our rigorously and well defined benchmarked programs compare to those based on more representative and real-world implementations. Indeed our results show that the rankings do not change apart from one programming language.
Keywords: Energy efficiency; Programming languages; Language benchmarking; Green software
