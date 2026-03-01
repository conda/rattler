import pathlib
import sys
import tempfile

import pytest

from rattler import PackageCache, ValidationMode


def test_single_dir() -> None:
    with tempfile.TemporaryDirectory() as d:
        cache = PackageCache(pathlib.Path(d))
        assert cache is not None


def test_single_dir_with_options() -> None:
    with tempfile.TemporaryDirectory() as d:
        cache = PackageCache(
            pathlib.Path(d),
            cache_origin=True,
            validation_mode=ValidationMode.Full,
        )
        assert cache is not None
        assert cache.paths()[0] == pathlib.Path(d)


def test_layered_dirs() -> None:
    with tempfile.TemporaryDirectory() as d1, tempfile.TemporaryDirectory() as d2:
        cache = PackageCache.new_layered([pathlib.Path(d1), pathlib.Path(d2)])
        assert cache is not None


def test_layered_dirs_with_options() -> None:
    with tempfile.TemporaryDirectory() as d1, tempfile.TemporaryDirectory() as d2:
        cache = PackageCache.new_layered(
            [pathlib.Path(d1), pathlib.Path(d2)],
            cache_origin=True,
            validation_mode=ValidationMode.Fast,
        )
        paths = cache.paths()
        assert len(paths) == 2
        assert paths[0] == pathlib.Path(d1)
        assert paths[1] == pathlib.Path(d2)


def test_paths_single() -> None:
    with tempfile.TemporaryDirectory() as d:
        cache = PackageCache(pathlib.Path(d))
        assert len(cache.paths()) == 1
        assert cache.paths()[0] == pathlib.Path(d)


def test_paths_layered_preserves_insertion_order() -> None:
    with tempfile.TemporaryDirectory() as d1, tempfile.TemporaryDirectory() as d2:
        cache = PackageCache.new_layered([pathlib.Path(d1), pathlib.Path(d2)])
        paths = cache.paths()
        assert len(paths) == 2
        assert paths[0] == pathlib.Path(d1)
        assert paths[1] == pathlib.Path(d2)


def test_writable_paths() -> None:
    with tempfile.TemporaryDirectory() as d:
        cache = PackageCache(pathlib.Path(d))
        writable = cache.writable_paths()
        assert pathlib.Path(d) in writable
        assert len(cache.readonly_paths()) == 0


@pytest.mark.skipif(
    sys.platform == "win32",
    reason="readonly dir permissions are not reliable on Windows",
)
def test_readonly_paths() -> None:
    with tempfile.TemporaryDirectory() as d1, tempfile.TemporaryDirectory() as d2:
        pathlib.Path(d1).chmod(0o555)
        try:
            cache = PackageCache.new_layered([pathlib.Path(d1), pathlib.Path(d2)])
            assert pathlib.Path(d1) in cache.readonly_paths()
            assert pathlib.Path(d1) not in cache.writable_paths()
            assert pathlib.Path(d2) in cache.writable_paths()
            assert pathlib.Path(d2) not in cache.readonly_paths()
        finally:
            pathlib.Path(d1).chmod(0o755)


def test_repr_single() -> None:
    with tempfile.TemporaryDirectory() as d:
        cache = PackageCache(pathlib.Path(d))
        r = repr(cache)
        assert "PackageCache" in r
        assert d in r


def test_repr_layered() -> None:
    with tempfile.TemporaryDirectory() as d1, tempfile.TemporaryDirectory() as d2:
        cache = PackageCache.new_layered([pathlib.Path(d1), pathlib.Path(d2)])
        r = repr(cache)
        assert d1 in r
        assert d2 in r


@pytest.mark.skipif(
    sys.platform == "win32",
    reason="readonly dir permissions are not reliable on Windows",
)
def test_paths_preserves_order_with_mixed_permissions() -> None:
    with tempfile.TemporaryDirectory() as d1, tempfile.TemporaryDirectory() as d2, \
         tempfile.TemporaryDirectory() as d3:
        pathlib.Path(d2).chmod(0o555)
        try:
            cache = PackageCache.new_layered([
                pathlib.Path(d1), pathlib.Path(d2), pathlib.Path(d3)
            ])
            paths = cache.paths()
            assert paths == [pathlib.Path(d1), pathlib.Path(d2), pathlib.Path(d3)]
        finally:
            pathlib.Path(d2).chmod(0o755)


def test_validation_mode_enum_values() -> None:
    assert ValidationMode.Skip is not None
    assert ValidationMode.Fast is not None
    assert ValidationMode.Full is not None
    assert len(ValidationMode) == 3
