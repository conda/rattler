import pathlib
import sys
import tempfile

import pytest

from rattler import PackageCache, ValidationMode


def test_single_dir_paths() -> None:
    with tempfile.TemporaryDirectory() as d:
        cache = PackageCache(pathlib.Path(d))
        assert cache.paths() == [pathlib.Path(d)]


def test_single_dir_with_options_paths() -> None:
    with tempfile.TemporaryDirectory() as d:
        cache = PackageCache(
            pathlib.Path(d),
            cache_origin=True,
            validation_mode=ValidationMode.Full,
        )
        assert cache.paths() == [pathlib.Path(d)]


def test_layered_dirs_preserves_insertion_order() -> None:
    with tempfile.TemporaryDirectory() as d1, tempfile.TemporaryDirectory() as d2:
        cache = PackageCache.new_layered([pathlib.Path(d1), pathlib.Path(d2)])
        paths = cache.paths()
        assert paths == [pathlib.Path(d1), pathlib.Path(d2)]


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


def test_writable_paths_includes_normal_dir() -> None:
    with tempfile.TemporaryDirectory() as d:
        cache = PackageCache(pathlib.Path(d))
        assert pathlib.Path(d) in cache.writable_paths()


def test_writable_dir_has_no_readonly_paths() -> None:
    with tempfile.TemporaryDirectory() as d:
        cache = PackageCache(pathlib.Path(d))
        assert cache.readonly_paths() == []


@pytest.mark.skipif(
    sys.platform == "win32",
    reason="readonly dir permissions are not reliable on Windows",
)
def test_readonly_layer_is_not_in_writable_paths() -> None:
    with tempfile.TemporaryDirectory() as d1, tempfile.TemporaryDirectory() as d2:
        pathlib.Path(d1).chmod(0o555)
        try:
            cache = PackageCache.new_layered([pathlib.Path(d1), pathlib.Path(d2)])
            assert pathlib.Path(d1) in cache.readonly_paths()
            assert pathlib.Path(d1) not in cache.writable_paths()
        finally:
            pathlib.Path(d1).chmod(0o755)


@pytest.mark.skipif(
    sys.platform == "win32",
    reason="readonly dir permissions are not reliable on Windows",
)
def test_writable_layer_is_not_in_readonly_paths() -> None:
    with tempfile.TemporaryDirectory() as d1, tempfile.TemporaryDirectory() as d2:
        pathlib.Path(d1).chmod(0o555)
        try:
            cache = PackageCache.new_layered([pathlib.Path(d1), pathlib.Path(d2)])
            assert pathlib.Path(d2) in cache.writable_paths()
            assert pathlib.Path(d2) not in cache.readonly_paths()
        finally:
            pathlib.Path(d1).chmod(0o755)


@pytest.mark.skipif(
    sys.platform == "win32",
    reason="readonly dir permissions are not reliable on Windows",
)
def test_paths_preserves_order_with_mixed_permissions() -> None:
    with tempfile.TemporaryDirectory() as d1, tempfile.TemporaryDirectory() as d2, tempfile.TemporaryDirectory() as d3:
        pathlib.Path(d2).chmod(0o555)
        try:
            cache = PackageCache.new_layered([pathlib.Path(d1), pathlib.Path(d2), pathlib.Path(d3)])
            assert cache.paths() == [
                pathlib.Path(d1),
                pathlib.Path(d2),
                pathlib.Path(d3),
            ]
        finally:
            pathlib.Path(d2).chmod(0o755)


def test_repr_contains_path() -> None:
    with tempfile.TemporaryDirectory() as d:
        cache = PackageCache(pathlib.Path(d))
        assert d in repr(cache)
        assert repr(cache).startswith("PackageCache(paths=")


def test_repr_layered_contains_all_paths() -> None:
    with tempfile.TemporaryDirectory() as d1, tempfile.TemporaryDirectory() as d2:
        cache = PackageCache.new_layered([pathlib.Path(d1), pathlib.Path(d2)])
        r = repr(cache)
        assert d1 in r
        assert d2 in r


def test_writable_paths_preserve_order() -> None:
    with tempfile.TemporaryDirectory() as d1, tempfile.TemporaryDirectory() as d2:
        cache = PackageCache.new_layered([pathlib.Path(d1), pathlib.Path(d2)])
        writable = cache.writable_paths()
        assert writable == [pathlib.Path(d1), pathlib.Path(d2)]
