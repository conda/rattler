import os
import stat
from pathlib import Path

import pytest

from rattler import CacheIndex, MatchSpec, PackageCache, PackageCacheLayer, ValidationMode


def test_validation_mode() -> None:
    skip = ValidationMode.skip()
    assert skip.is_skip
    assert not skip.is_fast
    assert not skip.is_full

    fast = ValidationMode.fast()
    assert fast.is_fast
    assert not fast.is_skip

    full = ValidationMode.full()
    assert full.is_full

    assert ValidationMode.full() == ValidationMode.full()
    assert ValidationMode.full() != ValidationMode.skip()
    assert hash(ValidationMode.full()) == hash(ValidationMode.full())

    d = {ValidationMode.full(): "full", ValidationMode.skip(): "skip"}
    assert d[ValidationMode.full()] == "full"
    assert d[ValidationMode.skip()] == "skip"

    assert ValidationMode.full() != "full"
    with pytest.raises(TypeError):
        ValidationMode.full() < ValidationMode.skip()  # noqa: B015  # ty: ignore[unsupported-operator]

    with pytest.raises(NotImplementedError):
        ValidationMode()


def test_package_cache_layer(tmp_path: Path) -> None:
    layer = PackageCacheLayer(tmp_path)
    assert layer.path == tmp_path
    assert not layer.is_readonly()
    assert "PackageCacheLayer" in repr(layer)

    layer_with_mode = layer.with_validation_mode(ValidationMode.full())
    assert layer_with_mode.path == tmp_path

    spec = MatchSpec("python >=3.9")
    layer_filtered = layer.with_filter(spec)
    assert layer_filtered.path == tmp_path

    layer_excluded = layer.excluding(spec)
    assert layer_excluded.path == tmp_path


@pytest.mark.asyncio
async def test_package_cache(tmp_path: Path) -> None:
    cache = PackageCache(tmp_path)
    assert len(cache.layers()) == 1
    assert cache.layers()[0].path == tmp_path
    assert cache.first_writable() == tmp_path

    cache_with_origin = cache.with_cached_origin()
    assert len(cache_with_origin.layers()) == 1

    spec = MatchSpec("python")
    cache_excluded = cache.excluding_from_all_layers(spec)
    assert len(cache_excluded.layers()) == 1

    index = await cache.index()
    assert isinstance(index, CacheIndex)
    assert index.is_empty()
    assert len(index) == 0


@pytest.mark.asyncio
async def test_package_cache_from_layers(tmp_path: Path) -> None:
    dir1 = tmp_path / "cache1"
    dir2 = tmp_path / "cache2"
    dir1.mkdir()
    dir2.mkdir()

    # Make dir2 read-only to ensure ordering is preserved even with mixed permissions
    os.chmod(dir2, stat.S_IREAD | stat.S_IEXEC)
    try:
        layer1 = PackageCacheLayer(dir1)
        layer2 = PackageCacheLayer(dir2)

        cache1 = PackageCache.from_layers([layer1, layer2])
        assert [layer.path for layer in cache1.layers()] == [dir1, dir2]

        cache2 = PackageCache.from_layers([layer2, layer1])
        assert [layer.path for layer in cache2.layers()] == [dir2, dir1]

        index = await cache1.index()
        assert index.is_empty()
        assert len(index) == 0
    finally:
        os.chmod(dir2, stat.S_IWRITE | stat.S_IREAD | stat.S_IEXEC)


def test_package_cache_new_layered(tmp_path: Path) -> None:
    dir1 = tmp_path / "cache1"
    dir2 = tmp_path / "cache2"
    dir1.mkdir()
    dir2.mkdir()

    cache = PackageCache.new_layered([dir1, dir2], cache_origin=True)
    assert len(cache.layers()) == 2
    assert cache.layers()[0].path == dir1
    assert cache.layers()[1].path == dir2
