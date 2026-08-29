from __future__ import annotations

import platform
import sys
from time import perf_counter_ns
from typing import Callable

from adaptive_contracts import ContractError, fixture_from_identifier, parse_and_validate_result


def format_duration(nanoseconds: int) -> str:
    if nanoseconds >= 1_000_000_000:
        return f"{nanoseconds / 1_000_000_000:.3f} s"
    if nanoseconds >= 1_000_000:
        return f"{nanoseconds / 1_000_000:.3f} ms"
    if nanoseconds >= 1_000:
        return f"{nanoseconds / 1_000:.3f} μs"
    return f"{nanoseconds:.3f} ns"


def percentile_nearest_rank(sorted_samples: list[int], percentile: int) -> int:
    rank = (len(sorted_samples) * percentile + 99) // 100
    index = max(0, min(rank - 1, len(sorted_samples) - 1))
    return sorted_samples[index]


def main() -> None:
    source_path = (
        sys.argv[1]
        if len(sys.argv) >= 2
        else "examples/sum_large.py"
    )
    warmup = int(sys.argv[2]) if len(sys.argv) >= 3 else 50
    iterations = int(sys.argv[3]) if len(sys.argv) >= 4 else 100

    if warmup < 0:
        raise ContractError("warmup must be non-negative")
    if iterations <= 0:
        raise ContractError("iterations must be positive")

    with open(source_path, "r", encoding="utf-8") as file:
        source = file.read()

    namespace: dict[str, str | Callable[[], int | float]] = {
        "__name__": "wustite_benchmark_target",
    }

    compile_started = perf_counter_ns()
    code = compile(source, source_path, "exec")
    exec(code, namespace)
    frontend_time = perf_counter_ns() - compile_started

    target = namespace.get("main")
    if not callable(target):
        raise ContractError("source does not define a callable main()")

    fixture = fixture_from_identifier(source_path)

    cold_started = perf_counter_ns()
    cold_result = target()
    cold_time = perf_counter_ns() - cold_started

    validated_cold = parse_and_validate_result(fixture, repr(cold_result))
    print(f"Cold sample: result={validated_cold!r} validated=true")

    for index in range(warmup):
        result = target()
        validated = parse_and_validate_result(fixture, repr(result))
        print(f"Warmup sample {index + 1}: result={validated!r} validated=true")

    samples: list[int] = []

    for index in range(iterations):
        started = perf_counter_ns()
        result = target()
        elapsed = perf_counter_ns() - started
        validated = parse_and_validate_result(fixture, repr(result))
        print(
            f"Measured sample {index + 1}: result={validated!r} "
            f"duration={format_duration(elapsed)} validated=true"
        )

        samples.append(elapsed)

    samples.sort()

    median = samples[len(samples) // 2]
    p95 = percentile_nearest_rank(samples, 95)

    print(f"Runtime: {platform.python_implementation()}")
    print(f"Version: {platform.python_version()}")
    print(f"Executable: {sys.executable}")
    print(f"Platform: {platform.platform()}")
    print(f"Benchmark: {source_path}")
    print(f"Warmup runs: {warmup}")
    print(f"Measured iterations: {iterations}")
    print()
    print(f"Source compile/load: {format_duration(frontend_time)}")
    print(f"Cold execution:     {format_duration(cold_time)}")
    print()
    print("Warm execution:")
    print(f"  Median: {format_duration(median)}")
    print(f"  P95:    {format_duration(p95)}")
    print(f"  Min:    {format_duration(samples[0])}")
    print(f"  Max:    {format_duration(samples[-1])}")


if __name__ == "__main__":
    main()
