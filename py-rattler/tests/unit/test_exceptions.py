import pytest
from rattler import SolverError, InvalidMatchSpecError, MatchSpec, solve, Gateway, Channel


@pytest.mark.asyncio
async def test_solver_error(gateway: Gateway, conda_forge_channel: Channel) -> None:
    # Try to solve for a non-existent package
    with pytest.raises(SolverError) as excinfo:
        await solve(
            [conda_forge_channel],
            ["non-existent-package-name-12345"],
            platforms=["linux-64"],
            gateway=gateway,
        )
    assert "Cannot solve" in str(excinfo.value)


def test_invalid_match_spec_error() -> None:
    # Try to create a MatchSpec with invalid syntax
    with pytest.raises(InvalidMatchSpecError) as excinfo:
        MatchSpec("invalid[[matchspec")
    assert "is not a valid package name" in str(excinfo.value).lower()


@pytest.mark.asyncio
async def test_solve_with_invalid_channel(gateway: Gateway) -> None:
    # Try to solve with an invalid channel URL
    with pytest.raises(Exception):  # Might be InvalidUrlError or something else depending on where it fails
        await solve(
            ["https://invalid.url/channel"],
            ["python"],
            platforms=["linux-64"],
            gateway=gateway,
        )
