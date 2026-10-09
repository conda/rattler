from pathlib import Path

import pytest

from rattler import CacheIndex, PackageCache, PackageCacheLayer, ValidationMode


def test_validation_mode() -> None:
    skip = ValidationMode("skip")
    assert skip.is_skip
    assert not skip.is_fast
    assert not skip.is_full

    fast = ValidationMode.fast()
    assert fast.is_fast
    assert not fast.is_skip

    full = ValidationMode.full()
    assert full.is_full

    with pytest.raises(ValueError, match="Invalid validation mode"):
        ValidationMode("invalid_mode")


def test_package_cache_layer(tmp_path: Path) -> None:
    layer = PackageCacheLayer(tmp_path)
    assert layer.path == tmp_path
    assert not layer.is_readonly
    assert "PackageCacheLayer" in repr(layer)

    layer_with_mode = layer.with_validation_mode(ValidationMode.full())
    assert layer_with_mode.path == tmp_path


@pytest.mark.asyncio
async def test_package_cache(tmp_path: Path) -> None:
    cache = PackageCache(tmp_path)
    assert len(cache.layers) == 1
    assert cache.layers[0].path == tmp_path
    assert cache.first_writable == tmp_path

    cache_with_origin = cache.with_cached_origin()
    assert len(cache_with_origin.layers) == 1

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

    layer1 = PackageCacheLayer(dir1)
    layer2 = PackageCacheLayer(dir2)

    cache = PackageCache.from_layers([layer1, layer2])
    assert len(cache.layers) == 2
    assert cache.layers[0].path == dir1
    assert cache.layers[1].path == dir2

    index = await cache.index()
    assert index.is_empty()
    assert len(index) == 0


def test_package_cache_new_layered(tmp_path: Path) -> None:
    dir1 = tmp_path / "cache1"
    dir2 = tmp_path / "cache2"
    dir1.mkdir()
    dir2.mkdir()

    cache = PackageCache.new_layered([dir1, dir2], cache_origin=True)
    assert len(cache.layers) == 2
    assert cache.layers[0].path == dir1
    assert cache.layers[1].path == dir2
