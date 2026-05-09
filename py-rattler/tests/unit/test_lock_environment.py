from pathlib import Path
from rattler import Channel, Environment, Platform, LockFile


def test_environment_with_pypi_packages() -> None:
    """
    Verifies that the Environment constructor accepts pypi_packages,
    resolving issue #1387.
    """
    # 1. Arrange: Load a real lockfile that contains PyPI packages
    # FIX: Wrap the string in Path() to satisfy mypy
    lock_path = Path("../test-data/test.lock")
    lock_file = LockFile.from_path(lock_path)

    source_env = lock_file.default_environment()

    # FIX: Assert is not None so mypy knows it is safe to use
    assert source_env is not None, "Default environment not found in lockfile"

    # 2. Extract a valid PypiLockedPackage from the existing lockfile
    pypi_packages = source_env.pypi_packages()
    assert len(pypi_packages) > 0, "Test lockfile does not contain PyPI packages"

    # 3. Act: Create a new environment using the extracted PyPI package
    try:
        new_env = Environment(
            name="test-env-with-pypi",
            requirements={},
            channels=[Channel("conda-forge")],
            pypi_packages=pypi_packages,
        )
    except TypeError as e:
        assert False, f"Environment constructor failed to accept pypi_packages: {e}"

    # 4. Assert: Check if the PyPI packages were actually added to the new environment
    pypi_in_new_env = new_env.pypi_packages_for_platform(Platform("osx-arm64"))
    assert pypi_in_new_env is not None
    assert len(pypi_in_new_env) > 0

    # Verify that at least one package name matches to prove success
    original_names = {pkg.name for pkg in pypi_packages[Platform("osx-arm64")]}
    new_names = {pkg.name for pkg in pypi_in_new_env}
    assert original_names == new_names
