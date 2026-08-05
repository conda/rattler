"""Regenerates the .conda fixtures in this directory.

Run from the repository root: python test-data/sparse/generate.py
Requires the `zstandard` package. Output is deterministic (seeded RNG,
fixed mtimes) up to the zstd library version.
"""

import io
import json
import random
import tarfile
import zipfile
from pathlib import Path

import zstandard

OUT = Path(__file__).parent


def tar_zst(entries, level=3):
    buf = io.BytesIO()
    with tarfile.open(fileobj=buf, mode="w") as tar:
        for name, data, link in entries:
            info = tarfile.TarInfo(name)
            info.mtime = 0
            if link:
                kind, target = link
                info.type = kind
                info.linkname = target
            else:
                info.size = len(data)
            tar.addfile(info, io.BytesIO(data) if not link else None)
    return zstandard.ZstdCompressor(level=level).compress(buf.getvalue())


def index_json(name):
    return json.dumps(
        {"name": name, "version": "1.0.0", "build": "0", "build_number": 0}
    ).encode()


def write_conda(stem, pkg_entries, info_entries, force_zip64=False):
    path = OUT / f"{stem}.conda"
    with zipfile.ZipFile(path, "w", compression=zipfile.ZIP_STORED) as zf:
        members = []
        if pkg_entries is not None:
            members.append((f"pkg-{stem}.tar.zst", tar_zst(pkg_entries)))
        members.append((f"info-{stem}.tar.zst", tar_zst(info_entries)))
        for name, data in members:
            if force_zip64:
                with zf.open(zipfile.ZipInfo(name), "w", force_zip64=True) as f:
                    f.write(data)
            else:
                zf.writestr(name, data)
    print(f"{path}: {path.stat().st_size} bytes")


# Larger than the 64 KiB tail so the payload member requires a ranged GET.
rng = random.Random(2252)
blob = bytes(rng.getrandbits(8) for _ in range(150_000))
write_conda(
    "sparse-test-1.0.0-0",
    [
        ("bin/first-file.txt", b"first payload file\n", None),
        ("lib/blob.bin", blob, None),
        ("share/last-file.txt", b"last payload file\n", None),
    ],
    [
        ("info/index.json", index_json("sparse-test"), None),
        (
            "info/paths.json",
            json.dumps(
                {
                    "paths_version": 1,
                    "paths": [
                        {"_path": "bin/first-file.txt", "path_type": "hardlink", "size_in_bytes": 19},
                        {"_path": "lib/blob.bin", "path_type": "hardlink", "size_in_bytes": 150000},
                        {"_path": "share/last-file.txt", "path_type": "hardlink", "size_in_bytes": 18},
                    ],
                }
            ).encode(),
            None,
        ),
    ],
)

# A payload containing a symbolic and a hard link.
write_conda(
    "symlink-test-1.0.0-0",
    [
        ("lib/libreal.so.1", b"real library bytes", None),
        ("lib/liblink.so", b"", (tarfile.SYMTYPE, "libreal.so.1")),
        ("lib/libhard.so", b"", (tarfile.LNKTYPE, "lib/libreal.so.1")),
    ],
    [("info/index.json", index_json("symlink-test"), None)],
)

# No pkg-*.tar.zst member at all.
write_conda(
    "info-only-1.0.0-0",
    None,
    [("info/index.json", index_json("info-only"), None)],
)

# zip64 local headers (sizes deferred to the zip64 extra field).
write_conda(
    "zip64-test-1.0.0-0",
    [("bin/hello.txt", b"zip64 payload\n", None)],
    [("info/index.json", index_json("zip64-test"), None)],
    force_zip64=True,
)

# Tar entries stored with a leading `./`, as produced by `tar -C dir -c .`.
write_conda(
    "dotslash-test-1.0.0-0",
    [("./lib/data.txt", b"dot slash payload\n", None)],
    [("./info/index.json", index_json("dotslash-test"), None)],
)
