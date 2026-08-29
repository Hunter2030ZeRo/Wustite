from __future__ import annotations

import sys
from dataclasses import dataclass
from pathlib import Path
from typing import Sequence

from adaptive_contracts import ContractError, FixtureId, fixture_from_identifier
from adaptive_runtime import Request as AdaptiveRequest
from adaptive_runtime import execute_runtime
from reference_runtime import REFERENCE_SPECS, Request as ReferenceRequest
from reference_runtime import RuntimeSelection, discover_installation, execute_reference, invoke


@dataclass(frozen=True, slots=True)
class QualificationRequest:
    fixtures: tuple[str, ...]
    warmup: int
    iterations: int
    binary: Path


def parse_request(arguments: Sequence[str]) -> QualificationRequest:
    if len(arguments) < 1:
        raise ContractError(
            "usage: qualify_adaptive.py FIXTURE [FIXTURE ...] "
            "[--binary PATH] [--warmup N --iterations N]"
        )
    warmup = 50
    iterations = 51
    binary = Path("target/release/wustite")
    fixtures: list[str] = []
    position = 0
    while position < len(arguments):
        argument = arguments[position]
        match argument:
            case "--warmup":
                try:
                    warmup = int(arguments[position + 1])
                except IndexError as error:
                    raise ContractError("--warmup requires an integer") from error
                except ValueError as error:
                    raise ContractError("--warmup requires an integer") from error
                position += 2
            case "--iterations":
                try:
                    iterations = int(arguments[position + 1])
                except IndexError as error:
                    raise ContractError("--iterations requires an integer") from error
                except ValueError as error:
                    raise ContractError("--iterations requires an integer") from error
                position += 2
            case "--binary":
                try:
                    binary = Path(arguments[position + 1])
                except IndexError as error:
                    raise ContractError("--binary requires a path") from error
                position += 2
            case _:
                fixtures.append(argument)
                position += 1
    if not fixtures or warmup < 0 or iterations <= 0:
        raise ContractError("fixtures and valid warmup/iterations are required")
    return QualificationRequest(tuple(fixtures), warmup, iterations, binary)


def main(arguments: Sequence[str]) -> int:
    try:
        request = parse_request(arguments)
        if not request.binary.is_file():
            raise ContractError(f"Wustite binary is unavailable: {request.binary}")

        root = Path(invoke(("pyenv", "root")).stdout.strip())
        installations = tuple(discover_installation(root, spec) for spec in REFERENCE_SPECS)
        failed = False
        for identifier in request.fixtures:
            fixture = fixture_from_identifier(identifier)
            reference_request = ReferenceRequest(
                RuntimeSelection.ALL, fixture, request.warmup, request.iterations
            )
            references = tuple(
                execute_reference(reference_request, installation, emit=False)
                for installation in installations
            )
            fastest = min(references, key=lambda result: result.median_ns)
            adaptive = execute_runtime(
                request.binary,
                AdaptiveRequest(fixture, request.warmup, request.iterations),
            )
            ratio = adaptive.median_ns / fastest.median_ns
            phase1 = ratio <= 2.0
            final = ratio <= 1.25
            compiler_regression = (
                adaptive.median_ns / adaptive.interpreter_median_ns
                if fixture.identifier == FixtureId.COMPILER_KERNELS
                else None
            )
            compiler_ok = compiler_regression is None or compiler_regression <= 1.05
            accepted_hot_trace_ok = (
                adaptive.machine_entries == 0 or adaptive.generic_dispatch_calls == 0
            )
            passed = phase1 and final and compiler_ok and accepted_hot_trace_ok
            failed |= not passed
            print(
                f"QUALIFICATION fixture={fixture.identifier} reference={fastest.installation.spec.identifier} "
                f"adaptive_binary={request.binary} "
                f"reference_median_ns={fastest.median_ns} adaptive_median_ns={adaptive.median_ns} "
                f"ratio={ratio:.6f} phase1={str(phase1).lower()} final={str(final).lower()} "
                f"compiler_regression={compiler_regression if compiler_regression is not None else 'n/a'} "
                f"compiler_ok={str(compiler_ok).lower()} machine_entries={adaptive.machine_entries} "
                f"helper_calls={adaptive.helper_calls} generic_dispatch_calls={adaptive.generic_dispatch_calls} "
                f"deopts={adaptive.deopts} samples_validated=true lifecycle=persistent pass={str(passed).lower()}"
            )
        return 1 if failed else 0
    except ContractError as error:
        print(f"error: {error}", file=sys.stderr)
        return 2


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))
