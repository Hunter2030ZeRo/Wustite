# Wustite 
<img width="2172" height="724" alt="ChatGPT Image 2026년 7월 26일 오후 10_38_39" src="https://github.com/user-attachments/assets/3f824541-9591-494e-a7fd-b8e854c571f2" />

**Wustite** is an end-to-end runtime for Python, based on register-based VM and pre-analysis augmented JIT compilation. 

## Why does Wustite matter?

CPython is known to consume up to 75x more energy and run up to 76x slower 
than native C code [[1](#1)]. Alternative runtimes have tried to close 
this gap via JIT compilation — PyPy pioneered the approach but development 
has largely stalled, while GraalPy remains active but suffers from poor 
C extension compatibility (NumPy, pandas, etc.) due to its non-CPython 
memory model.

Wustite takes a different approach. Instead of relying on runtime profiling 
to warm up a JIT, Wustite builds a **structure map** ahead of time that lets 
the runtime skip the warm-up phase entirely and eliminates interpretation 
overhead at execution. This is paired with **WVM**, a CPython-compatible 
virtual machine — so Wustite aims to deliver both the performance of 
ahead-of-time compilation and the efficiency of a runtime that doesn't 
sacrifice compatibility to get there.


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
