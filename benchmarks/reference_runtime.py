from __future__ import annotations

import subprocess
import sys
from dataclasses import dataclass
from enum import StrEnum
from pathlib import Path
from typing import Final, Sequence, assert_never

from adaptive_contracts import ContractError, FixtureContract, fixture_from_identifier, parse_and_validate_result


class RuntimeId(StrEnum):
    PYPY = "pypy"
    GRAALPY = "graalpy"


class RuntimeSelection(StrEnum):
    ALL = "all"
    PYPY = RuntimeId.PYPY
    GRAALPY = RuntimeId.GRAALPY


@dataclass(frozen=True, slots=True)
class RuntimeSpec:
    identifier: RuntimeId
    installation: str
    executable: str
    implementation: str
    python_version: str
    engine_version: str


@dataclass(frozen=True, slots=True)
class RuntimeIdentity:
    implementation: str
    python_version: str
    engine_version: str


@dataclass(frozen=True, slots=True)
class Installation:
    spec: RuntimeSpec
    executable: Path
    identity: RuntimeIdentity


@dataclass(frozen=True, slots=True)
class Request:
    selection: RuntimeSelection
    fixture: FixtureContract
    warmup: int
    iterations: int


@dataclass(frozen=True, slots=True)
class Sample:
    duration_ns: int
    value: int | float


@dataclass(frozen=True, slots=True)
class RuntimeResult:
    installation: Installation
    median_ns: int


REFERENCE_SPECS: Final[tuple[RuntimeSpec, ...]] = (
    RuntimeSpec(RuntimeId.PYPY, "pypy3.11-7.3.22", "pypy3", "pypy", "3.11.15", "7.3.22"),
    RuntimeSpec(RuntimeId.GRAALPY, "graalpy-25.0.3", "graalpy", "graalpy", "3.12.8", "25.0.3"),
)
IDENTITY_PROGRAM: Final[str] = (
    "import sys\n"
    "engine = sys.implementation.version\n"
    "print(f'{sys.implementation.name}|{sys.version_info.major}.{sys.version_info.minor}.{sys.version_info.micro}|{engine.major}.{engine.minor}.{engine.micro}')\n"
)
RUNNER_PROGRAM: Final[str] = (
    "import runpy, sys\n"
    "from time import perf_counter_ns\n"
    "source_path, warmup_text, iterations_text = sys.argv[1:]\n"
    "namespace = runpy.run_path(source_path, run_name='wustite_benchmark_target')\n"
    "target = namespace['main']\n"
    "def emit(phase, index):\n"
    "    started = perf_counter_ns()\n"
    "    result = target()\n"
    "    elapsed = perf_counter_ns() - started\n"
    "    print(f'{phase}\\t{index}\\t{elapsed}\\t{result!r}')\n"
    "emit('cold', 1)\n"
    "for index in range(int(warmup_text)):\n"
    "    emit('warmup', index + 1)\n"
    "for index in range(int(iterations_text)):\n"
    "    emit('measured', index + 1)\n"
)


def parse_request(arguments: Sequence[str]) -> Request:
    if len(arguments) != 8 or arguments[0] != "--runtime" or arguments[2] != "--fixture" or arguments[4] != "--warmup" or arguments[6] != "--iterations":
        raise ContractError(
            "usage: reference_runtime.py --runtime {all,pypy,graalpy} --fixture PATH --warmup N --iterations N"
        )
    try:
        selection = RuntimeSelection(arguments[1])
        warmup = int(arguments[5])
        iterations = int(arguments[7])
    except ValueError as error:
        raise ContractError("runtime, warmup, or iterations is invalid") from error
    if warmup < 0 or iterations <= 0:
        raise ContractError("warmup must be non-negative and iterations positive")
    return Request(selection, fixture_from_identifier(arguments[3]), warmup, iterations)


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
    except subprocess.TimeoutExpired as error:
        raise ContractError(
            f"timed out after {timeout_seconds} seconds: {command[0]}"
        ) from error
    if completed.returncode != 0:
        raise ContractError(
            f"{command[0]} exited {completed.returncode}: {completed.stderr.strip()}"
        )
    return completed


def selected_specs(selection: RuntimeSelection) -> tuple[RuntimeSpec, ...]:
    match selection:
        case RuntimeSelection.ALL:
            return REFERENCE_SPECS
        case RuntimeSelection.PYPY:
            return (REFERENCE_SPECS[0],)
        case RuntimeSelection.GRAALPY:
            return (REFERENCE_SPECS[1],)
        case unreachable:
            assert_never(unreachable)


def discover_installation(root: Path, spec: RuntimeSpec) -> Installation:
    executable = root / "versions" / spec.installation / "bin" / spec.executable
    if not executable.is_file():
        raise ContractError(f"missing {spec.identifier} executable under pyenv root: {executable}")
    completed = invoke((str(executable), "-c", IDENTITY_PROGRAM))
    parts = completed.stdout.strip().split("|")
    if len(parts) != 3:
        raise ContractError(f"unparseable {spec.identifier} identity: {completed.stdout!r}")
    identity = RuntimeIdentity(parts[0], parts[1], parts[2])
    if (
        identity.implementation != spec.implementation
        or identity.python_version != spec.python_version
        or identity.engine_version != spec.engine_version
    ):
        raise ContractError(
            f"unexpected {spec.identifier} identity: {identity.implementation} "
            f"{identity.python_version} / {identity.engine_version}"
        )
    return Installation(spec, executable, identity)


def parse_samples(output: str, fixture: FixtureContract, request: Request) -> tuple[Sample, ...]:
    lines = tuple(line for line in output.splitlines() if line)
    expected_count = 1 + request.warmup + request.iterations
    if len(lines) != expected_count:
        raise ContractError(f"expected {expected_count} sample records, received {len(lines)}")
    samples: list[Sample] = []
    for position, line in enumerate(lines):
        parts = line.split("\t", 3)
        if len(parts) != 4:
            raise ContractError(f"malformed sample record: {line!r}")
        phase, index_text, duration_text, value_text = parts
        expected_phase = "cold" if position == 0 else "warmup" if position <= request.warmup else "measured"
        expected_index = 1 if position == 0 else position if position <= request.warmup else position - request.warmup
        if phase != expected_phase or index_text != str(expected_index):
            raise ContractError(f"unexpected sample order: {line!r}")
        try:
            duration_ns = int(duration_text)
        except ValueError as error:
            raise ContractError(f"malformed duration: {duration_text!r}") from error
        if duration_ns <= 0:
            raise ContractError(f"non-positive duration: {duration_ns}")
        samples.append(Sample(duration_ns, parse_and_validate_result(fixture, value_text)))
    return tuple(samples)


def execute_reference(
    request: Request, installation: Installation, *, emit: bool = True
) -> RuntimeResult:
    sample_count = 1 + request.warmup + request.iterations
    completed = invoke(
        (
            str(installation.executable),
            "-c",
            RUNNER_PROGRAM,
            str(request.fixture.identifier),
            str(request.warmup),
            str(request.iterations),
        ),
        timeout_seconds=max(120, sample_count * 5),
    )
    samples = parse_samples(completed.stdout, request.fixture, request)
    if emit:
        print(
            f"RUNTIME id={installation.spec.identifier} implementation={installation.identity.implementation} "
            f"python={installation.identity.python_version} engine={installation.identity.engine_version} "
            f"executable={installation.executable}"
        )
    measured = samples[1 + request.warmup :]
    for position, sample in enumerate(samples):
        phase = "cold" if position == 0 else "warmup" if position <= request.warmup else "measured"
        index = 1 if position == 0 else position if position <= request.warmup else position - request.warmup
        if emit:
            print(
                f"SAMPLE runtime={installation.spec.identifier} phase={phase} index={index} "
                f"valid=true result={sample.value!r} duration_ns={sample.duration_ns}"
            )
    ordered = sorted(sample.duration_ns for sample in measured)
    return RuntimeResult(installation, ordered[len(ordered) // 2])


def main(arguments: Sequence[str]) -> int:
    try:
        request = parse_request(arguments)
        root = Path(invoke(("pyenv", "root")).stdout.strip())
        results = tuple(
            execute_reference(request, discover_installation(root, spec))
            for spec in selected_specs(request.selection)
        )
        for result in results:
            print(
                f"MEDIAN runtime={result.installation.spec.identifier} "
                f"fixture={request.fixture.identifier} duration_ns={result.median_ns}"
            )
        fastest = min(results, key=lambda result: result.median_ns)
        print(
            f"FASTEST_CORRECT fixture={request.fixture.identifier} "
            f"runtime={fastest.installation.spec.identifier} median_ns={fastest.median_ns}"
        )
        return 0
    except ContractError as error:
        print(f"error: {error}", file=sys.stderr)
        return 2


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))
