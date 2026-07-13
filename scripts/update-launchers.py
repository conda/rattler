#!/usr/bin/env python3
"""Refresh the vendored Windows entry-point launchers.

`rattler` embeds three Windows launchers (see
`crates/rattler/src/install/entry_point.rs`) and copies them next to every
`*-script.py` in a prefix's `Scripts` directory to proxy Python
`console_scripts` entry points to `python.exe` (shebangs do not work on
Windows).

The launchers are the **code-signed** release assets published by
[`conda/conda-launchers`](https://github.com/conda/conda-launchers/releases)
(a CPython 3.7 launcher patched for the conda ecosystem, signed with Azure
Trusted Signing). This script downloads a pinned set, verifies them against
their published SHA-256, and writes them to `crates/rattler/resources/`.

The `LAUNCHERS` table below is the source of truth / provenance record. To
bump to a newer release, update the `release`, `asset`, and `sha256` fields
(asset names and checksums are on the conda-launchers release page) and re-run:

    python scripts/update-launchers.py
"""

from __future__ import annotations

import hashlib
import sys
import urllib.request
from pathlib import Path

REPO = "conda/conda-launchers"
RESOURCES = Path(__file__).resolve().parent.parent / "crates" / "rattler" / "resources"

# Vendored launchers: destination file -> (release tag, release asset, sha256).
#
# Variant choice (per conda-launchers' guidance):
#   - cli-64    -> gcc  (recommended, smallest proven mingw build)
#   - cli-32    -> zig  (only small build available for win-32)
#   - cli-arm64 -> zig  (small; the vs2022 build is ~1 MB per copy)
LAUNCHERS = {
    "cli-32.exe": (
        "24.7.1-5",
        "cli-32-24.7.1-90b98ef_zig_5.exe",
        "3856b8e5971a9238eae5608701fc2694dd2f2084122ce3ba8eae400fc2314599",
    ),
    "cli-64.exe": (
        "24.7.1-5",
        "cli-64-24.7.1-e8b2e36_gcc_5.exe",
        "4d8c479577961d0c6d940966f7987fbb0d6fc98b80edc03b00ee949dd8c1b3e2",
    ),
    "cli-arm64.exe": (
        "24.7.1-5",
        "cli-arm64-24.7.1-53f4ca0_zig_5.exe",
        "3833045196b8fcc6e1a58b92cd402af3db8752e9f7d9686e245fd217c08dd244",
    ),
}


def download(release: str, asset: str) -> bytes:
    url = f"https://github.com/{REPO}/releases/download/{release}/{asset}"
    print(f"Downloading {url}")
    with urllib.request.urlopen(url) as response:  # noqa: S310 (trusted URL)
        return response.read()


def main() -> int:
    errors = 0
    for dest, (release, asset, expected_sha) in LAUNCHERS.items():
        data = download(release, asset)
        actual_sha = hashlib.sha256(data).hexdigest()
        if actual_sha != expected_sha:
            print(
                f"  ERROR: sha256 mismatch for {asset}\n"
                f"    expected {expected_sha}\n"
                f"    got      {actual_sha}",
                file=sys.stderr,
            )
            errors += 1
            continue
        out = RESOURCES / dest
        out.write_bytes(data)
        print(f"  Wrote {out} ({len(data)} bytes, sha256 OK)")

    if errors:
        print(f"\n{errors} launcher(s) failed verification.", file=sys.stderr)
        return 1
    print("\nAll launchers updated and verified.")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
