from __future__ import annotations

from dataclasses import dataclass
from enum import StrEnum
from typing import Final, assert_never


class FixtureId(StrEnum):
    COMPILER_KERNELS = "benchmarks/compiler_kernels.py"
    SPECTRAL_NORM = "examples/spectral_norm.py"
    FANNKUCH = "examples/fannkuch.py"
    NBODY = "examples/nbody.py"
    SUM_LARGE = "examples/sum_large.py"
    SHAPE_OBJECTS = "benchmarks/adaptive_shape_objects.py"
    CALL_OBJECTS = "benchmarks/adaptive_call_objects.py"
    LIST_OBJECTS = "benchmarks/adaptive_list_objects.py"


@dataclass(frozen=True, slots=True)
class FixtureContract:
    identifier: FixtureId
    expected: int | float
    tolerance: float


class ContractError(Exception):
    def __init__(self, detail: str) -> None:
        self.detail = detail
        super().__init__(detail)


FIXTURES: Final[tuple[FixtureContract, ...]] = (
    FixtureContract(FixtureId.COMPILER_KERNELS, 2755, 0.0),
    FixtureContract(FixtureId.SPECTRAL_NORM, 1.6236422398020804, 1e-12),
    FixtureContract(FixtureId.FANNKUCH, 30, 0.0),
    FixtureContract(FixtureId.NBODY, -0.16908926275527172, 1e-12),
    FixtureContract(FixtureId.SUM_LARGE, 500_000_500_000, 0.0),
    FixtureContract(FixtureId.SHAPE_OBJECTS, 4096, 0.0),
    FixtureContract(FixtureId.CALL_OBJECTS, 24_512, 0.0),
    FixtureContract(FixtureId.LIST_OBJECTS, 2016, 0.0),
)


def fixture_from_identifier(identifier: str) -> FixtureContract:
    for fixture in FIXTURES:
        if fixture.identifier == identifier:
            return fixture
    raise ContractError(f"unknown benchmark fixture: {identifier}")


def parse_and_validate_result(fixture: FixtureContract, text: str) -> int | float:
    result_text = text.strip()
    if not result_text:
        raise ContractError(f"{fixture.identifier} produced an empty result")
    match fixture.expected:
        case expected if isinstance(expected, int):
            try:
                actual = int(result_text)
            except ValueError as error:
                raise ContractError(
                    f"{fixture.identifier} produced non-integer result {result_text!r}"
                ) from error
            if actual != expected:
                raise ContractError(
                    f"{fixture.identifier} produced {actual!r}; expected {expected!r}"
                )
            return actual
        case expected if isinstance(expected, float):
            try:
                actual = float(result_text)
            except ValueError as error:
                raise ContractError(
                    f"{fixture.identifier} produced non-float result {result_text!r}"
                ) from error
            if abs(actual - expected) > fixture.tolerance:
                raise ContractError(
                    f"{fixture.identifier} produced {actual!r}; expected {expected!r} "
                    f"within {fixture.tolerance}"
                )
            return actual
        case unreachable:
            assert_never(unreachable)
