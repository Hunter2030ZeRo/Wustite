from __future__ import annotations

import re
import subprocess
import sys
from dataclasses import dataclass
from pathlib import Path
from typing import Sequence

from adaptive_contracts import ContractError, FixtureContract, fixture_from_identifier


@dataclass(frozen=True, slots=True)
class Request:
    fixture: FixtureContract
    warmup: int
    iterations: int


@dataclass(frozen=True, slots=True)
class RuntimeResult:
    median_ns: int
    interpreter_median_ns: int
    machine_entries: int
    helper_calls: int
    generic_dispatch_calls: int
    deopts: int


def parse_request(arguments: Sequence[str]) -> Request:
    if len(arguments) != 6 or arguments[0] != "--fixture" or arguments[2] != "--warmup" or arguments[4] != "--iterations":
        raise ContractError(
            "usage: adaptive_runtime.py --fixture PATH --warmup N --iterations N"
        )
    try:
        warmup = int(arguments[3])
        iterations = int(arguments[5])
    except ValueError as error:
        raise ContractError("warmup and iterations must be integers") from error
    if warmup < 0 or iterations <= 0:
        raise ContractError("warmup must be non-negative and iterations positive")
    return Request(fixture_from_identifier(arguments[1]), warmup, iterations)


def invoke(
    command: tuple[str, ...], *, timeout_seconds: int = 120
) -> subprocess.CompletedProcess[str]:
    try:
        completed = subprocess.run(
            command,
            capture_output=True,
            text=True,
            timeout=timeout_seconds,
            check=False,
        )
    except FileNotFoundError as error:
        raise ContractError(f"missing executable: {command[0]}") from error
    except PermissionError as error:
        raise ContractError(f"permission denied: {command[0]}") from error
    except subprocess.TimeoutExpired as error:
        raise ContractError(
            f"timed out after {timeout_seconds} seconds: {command[0]}"
        ) from error
    if completed.returncode != 0:
        raise ContractError(
            f"{command[0]} exited {completed.returncode}: {completed.stderr.strip()}"
        )
    return completed


def execute_runtime(binary: Path, request: Request) -> RuntimeResult:
    command = (
        str(binary),
        "bench",
        str(request.fixture.identifier),
        "--warmup",
        str(request.warmup),
        "--iterations",
        str(request.iterations),
        "--interpreter-warmup",
        "0",
        "--interpreter-iterations",
        "1",
        "--backend",
        "cranelift",
        "--runtime-core",
        "adaptive-v2",
        "--jit-policy",
        "structure-map",
        "--hot-threshold",
        "10",
        "--debug-jit",
    )
    sample_count = 1 + request.warmup + request.iterations
    completed = invoke(command, timeout_seconds=max(120, sample_count * 5))
    warm = completed.stdout.partition("Adaptive JIT warm:")[2]
    median_match = re.search(r"^  Median ns: (\d+)$", warm, re.MULTILINE)
    if median_match is None:
        raise ContractError("Wustite benchmark omitted adaptive warm median")
    report_match = re.search(
        r"adaptive-v2 measured_delta machine_entries=(\d+) helper_calls=(\d+) "
        r"generic_dispatch_calls=(\d+) deopts=(\d+)",
        completed.stderr,
    )
    if report_match is None:
        raise ContractError("Wustite benchmark omitted adaptive-v2 schema-1 counters")
    interpreter = completed.stdout.partition("Interpreter:")[2].partition("Adaptive JIT cold:")[0]
    interpreter_match = re.search(r"^  Median ns: (\d+)$", interpreter, re.MULTILINE)
    if interpreter_match is None:
        raise ContractError("Wustite benchmark omitted interpreter median")
    return RuntimeResult(
        int(median_match.group(1)),
        int(interpreter_match.group(1)),
        *(int(value) for value in report_match.groups()),
    )


def main(arguments: Sequence[str]) -> int:
    try:
        request = parse_request(arguments)
        binary = Path("target/release/wustite")
        if not binary.is_file():
            raise ContractError(f"release binary is unavailable: {binary}")
        version = invoke((str(binary), "--version")).stdout.strip()
        print(f"RUNTIME name=Wustite version={version} executable={binary}")
        result = execute_runtime(binary, request)
        if result.machine_entries > 0 and result.generic_dispatch_calls != 0:
            raise ContractError("accepted hot trace used generic dispatch")
        print(
            f"COUNTERS machine_entries={result.machine_entries} helper_calls={result.helper_calls} "
            f"generic_dispatch_calls={result.generic_dispatch_calls} deopts={result.deopts}"
        )
        print(
            f"MEDIAN fixture={request.fixture.identifier} duration_ns={result.median_ns} "
            f"samples={request.iterations} samples_validated=true lifecycle=persistent"
        )
        return 0
    except ContractError as error:
        print(f"error: {error}", file=sys.stderr)
        return 2


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))
