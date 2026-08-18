#!/usr/bin/env python3
"""Rebuild the virtual package plugin fixture packages and their repodata.

The fixtures are real conda packages, so changing what one of them prints means
rebuilding an archive and updating the hashes that describe it: the ones in its
own `info/paths.json` and the ones in the channel's `noarch/repodata.json`.
Doing that by hand is how a fixture ends up with a hash that no longer matches
its contents.

Three kinds of package live in this channel:

- `foobar-detect`, the virtual package plugin this channel registers. It reports
  fixed verdicts so detection can be exercised without the hardware.
- `foobar-probe`, in two flavours, for checking by hand whether a virtual package
  reached the solver. See `probe_packages`.
- `change-me-detect` and `change-me-probe`, whose verdict changes with the clock.

Two sibling channels are written too, because what they demonstrate needs both
of them: `virtual-package-plugins-derived` declares
`virtual-package-plugins-base` as its CEP-42 base, and its plugin depends on a
package only the base channel serves. See `related_channel_packages`.

Run it from anywhere; it writes only inside these three channel directories.
"""

import hashlib
import io
import json
import tarfile
from dataclasses import dataclass
from pathlib import Path

CHANNEL = Path(__file__).resolve().parent
CHANNELS = CHANNEL.parent
BASE_CHANNEL = "virtual-package-plugins-base"
DERIVED_CHANNEL = "virtual-package-plugins-derived"
PLUGIN = "foobar-detect"
PROBE = "foobar-probe"
CHANGING = "change-me-detect"
CHANGING_PROBE = "change-me-probe"
VERSION = "1.0.0"

# Fixed so the archives are byte-for-byte reproducible.
TIMESTAMP = 1700000000

REPORT = {
    "version": 1,
    "virtual_packages": {
        "__foobar": {"version": "1.2.3"},
        "__foobar_arch": {"version": "0", "build_string": "gen4"},
    },
    "cache": {"ttl_seconds": 3600},
}

PLUGIN_PREAMBLE = (
    "Test fixture: reports fixed verdicts for the virtual packages this channel "
    f"registers {PLUGIN} for, so detection can be exercised without the hardware. "
    "Real plugins inspect the system here."
)


@dataclass(frozen=True)
class Package:
    """One fixture package: a `noarch: generic` archive and its repodata entry."""

    name: str
    build_string: str
    build_number: int
    depends: tuple[str, ...]
    files: dict[str, str]
    channel: str = CHANNEL.name

    @property
    def archive_name(self) -> str:
        return f"{self.name}-{VERSION}-{self.build_string}.tar.bz2"

    def index(self) -> dict[str, object]:
        return {
            "build": self.build_string,
            "build_number": self.build_number,
            "depends": list(self.depends),
            "name": self.name,
            "noarch": "generic",
            "subdir": "noarch",
            "timestamp": TIMESTAMP * 1000,
            "version": VERSION,
        }


def scripts(name: str, comment: str, message: str) -> dict[str, str]:
    """One entry point per platform, both printing `message`."""
    return {
        f"bin/{name}": (
            f"#!/bin/sh\n# {comment}\nprintf '%s\\n' '{message}'\nexit 0\n"
        ),
        f"Scripts/{name}.bat": (
            f"@echo off\r\nREM {comment}\r\necho {message}\r\nexit /b 0\r\n"
        ),
    }


def plugin_package() -> Package:
    """The virtual package plugin this channel registers."""
    return Package(
        name=PLUGIN,
        build_string="h0000000_0",
        build_number=0,
        depends=(),
        files=scripts(PLUGIN, PLUGIN_PREAMBLE, json.dumps(REPORT)),
    )


def changing_plugin_package() -> Package:
    """A plugin whose verdict changes with the clock, for watching a re-detect.

    Every other minute it reports `__change_me` as `0=even`, and in between as
    `1=odd`. Nothing else in this channel changes on its own, so this is what a
    cache expiry, a re-solve, or an override can be observed against.

    Its `ttl_seconds` is `0`, which makes each cache entry expire the instant it
    is written: a plugin that answers differently every minute must not have an
    answer kept for the default hour.

    `date +%M` is zero-padded, and a leading zero would make `08` an invalid
    octal number in `$(( ))`, so it is stripped before the arithmetic. The batch
    file takes the last digit instead, which has the same parity and cannot be
    read as octal either.
    """
    report = json.dumps(
        {
            "version": 1,
            "virtual_packages": {
                "__change_me": {"version": "%s", "build_string": "%s"}
            },
            "cache": {"ttl_seconds": 0},
        }
    )
    comment = (
        "Test fixture: reports __change_me as 0=even on even minutes and 1=odd on "
        "odd ones, so a re-detection has something to show."
    )
    return Package(
        name=CHANGING,
        build_string="h0000000_0",
        build_number=0,
        depends=(),
        files={
            f"bin/{CHANGING}": (
                "#!/bin/sh\n"
                f"# {comment}\n"
                "minute=$(date +%M)\n"
                "minute=${minute#0}\n"
                '[ -n "$minute" ] || minute=0\n'
                "if [ $(( minute % 2 )) -eq 0 ]; then\n"
                f"    printf '%s\\n' '{report % ('0', 'even')}'\n"
                "else\n"
                f"    printf '%s\\n' '{report % ('1', 'odd')}'\n"
                "fi\n"
                "exit 0\n"
            ),
            f"Scripts/{CHANGING}.bat": (
                "@echo off\r\n"
                f"REM {comment}\r\n"
                'for /f "tokens=2 delims=:" %%m in ("%TIME%") do set MINUTE=%%m\r\n'
                "set /a PARITY=%MINUTE:~-1% %% 2\r\n"
                "if %PARITY%==0 (\r\n"
                f"    echo {report % ('0', 'even')}\r\n"
                ") else (\r\n"
                f"    echo {report % ('1', 'odd')}\r\n"
                ")\r\n"
                "exit /b 0\r\n"
            ),
        },
    )


def changing_probe_packages() -> list[Package]:
    """Two flavours of one package, one installable per state of `__change_me`.

    Unlike `foobar-probe` these are mutually exclusive rather than ranked: `even`
    needs `__change_me ==0` and `odd` needs `==1`, so exactly one of them can be
    solved for at any moment and which one it is flips every minute. Installing
    `change-me-probe` and running it therefore shows the state the *solver* saw.
    """
    return [
        Package(
            name=CHANGING_PROBE,
            build_string=state,
            build_number=0,
            depends=(f"__change_me =={version}",),
            files=scripts(
                CHANGING_PROBE,
                f"Test fixture: the {state} flavour of {CHANGING_PROBE}.",
                f"__change_me was {version}={state} at solve time.",
            ),
        )
        for version, state in (("0", "even"), ("1", "odd"))
    ]


def probe_packages() -> list[Package]:
    """Two flavours of one package, telling you which one the solver could take.

    Both are `foobar-probe 1.0.0`. The only differences are that one depends on
    `__foobar` and the other does not, and that the depending one has the higher
    build number -- so a solver takes it whenever it can, and falls back to the
    other only when `__foobar` is missing.

    Running `foobar-probe` from the resulting environment therefore reports
    whether the virtual package reached the solver. Which is the point: the
    verdict is decided at solve time and merely read back afterwards, so it stays
    true even though the script itself checks nothing.
    """
    common = (
        f"Test fixture: one of the two {PROBE} builds. Which one a solve picks "
        "depends on whether __foobar was offered to the solver."
    )
    return [
        Package(
            name=PROBE,
            build_string="with_foobar",
            build_number=1,
            depends=("__foobar >=1.0",),
            files=scripts(
                PROBE,
                common,
                "__foobar WAS available at solve time "
                '(the "with_foobar" build was installable).',
            ),
        ),
        Package(
            name=PROBE,
            build_string="without_foobar",
            build_number=0,
            depends=(),
            files=scripts(
                PROBE,
                common,
                "__foobar was NOT available at solve time "
                '(fell back to the "without_foobar" build).',
            ),
        ),
    ]


def related_channel_packages() -> list[Package]:
    """A plugin in one channel and the library it needs in that channel's base.

    `virtual-package-plugins-derived` registers `vendor-cuda-detect` and serves
    it; `vendor-lib`, which it depends on, exists only in
    `virtual-package-plugins-base`, which the derived channel declares as its
    CEP-42 base. Installing the plugin therefore only succeeds if the relation
    is followed, which is what a plugin's dependencies are allowed to reach and
    the only thing they are allowed to reach.
    """
    marker = "share/vendor-lib/version"
    report = json.dumps({"version": 1, "virtual_packages": {"__cuda": {"version": "12.4"}}})
    return [
        Package(
            name="vendor-lib",
            build_string="h0000000_0",
            build_number=0,
            depends=(),
            files={marker: "1.0.0\\n"},
            channel=BASE_CHANNEL,
        ),
        Package(
            name="vendor-cuda-detect",
            build_string="h0000000_0",
            build_number=0,
            depends=("vendor-lib",),
            files=scripts(
                "vendor-cuda-detect",
                "Test fixture: a plugin whose dependency lives in the base channel.",
                report,
            ),
            channel=DERIVED_CHANNEL,
        ),
    ]


def sha256(payload: bytes) -> str:
    return hashlib.sha256(payload).hexdigest()


def build_archive(files: dict[str, bytes]) -> bytes:
    """A tarball with fixed metadata, so identical contents give an identical file."""
    raw = io.BytesIO()
    with tarfile.open(fileobj=raw, mode="w:bz2", format=tarfile.GNU_FORMAT) as archive:
        for name in sorted(files):
            info = tarfile.TarInfo(name)
            info.size = len(files[name])
            info.mtime = TIMESTAMP
            info.mode = 0o755 if name.startswith(("bin/", "Scripts/")) else 0o644
            info.uid = info.gid = 0
            info.uname = info.gname = ""
            archive.addfile(info, io.BytesIO(files[name]))
    return raw.getvalue()


def write(package: Package) -> dict[str, object]:
    """Writes the archive and returns the repodata entry describing it."""
    files = {name: body.encode() for name, body in package.files.items()}
    paths = {
        "paths": [
            {
                "_path": name,
                "path_type": "hardlink",
                "sha256": sha256(files[name]),
                "size_in_bytes": len(files[name]),
            }
            for name in sorted(files)
        ],
        "paths_version": 1,
    }
    files["info/index.json"] = (json.dumps(package.index(), indent=2) + "\n").encode()
    files["info/paths.json"] = (json.dumps(paths, indent=2) + "\n").encode()

    archive = build_archive(files)
    (CHANNELS / package.channel / "noarch" / package.archive_name).write_bytes(archive)
    print(f"wrote {package.channel}/noarch/{package.archive_name} ({len(archive)} bytes)")

    return package.index() | {
        "md5": hashlib.md5(archive).hexdigest(),
        "sha256": sha256(archive),
        "size": len(archive),
    }


def main() -> None:
    packages = [
        plugin_package(),
        *probe_packages(),
        changing_plugin_package(),
        *changing_probe_packages(),
        *related_channel_packages(),
    ]

    entries: dict[str, dict[str, dict[str, object]]] = {}
    for package in packages:
        entries.setdefault(package.channel, {})[package.archive_name] = write(package)

    for channel, channel_entries in entries.items():
        repodata_path = CHANNELS / channel / "noarch" / "repodata.json"
        repodata = json.loads(repodata_path.read_text())
        known = repodata.get("packages", {})
        repodata["packages"] = dict(sorted((known | channel_entries).items()))
        repodata_path.write_text(json.dumps(repodata, indent=2) + "\n")


if __name__ == "__main__":
    main()
