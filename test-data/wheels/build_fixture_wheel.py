"""Builds a small, deterministic pure-Python wheel used as a test fixture.

The package exercises every part of a wheel that the conda layout conversion
has to handle: a package directory, a console entry point, a .data/scripts
entry, a .data/data entry, and the dist-info metadata with a RECORD.
"""
import base64, hashlib, io, sys, zipfile

NAME, VERSION = "rattler_fixture", "1.0.0"
DIST_INFO = f"{NAME}-{VERSION}.dist-info"
DATA = f"{NAME}-{VERSION}.data"
STAMP = (2026, 1, 1, 0, 0, 0)

files = {
    f"{NAME}/__init__.py": b'"""Fixture package for rattler wheel tests."""\n\n__version__ = "1.0.0"\n\n\ndef main() -> None:\n    print("moo")\n',
    f"{NAME}/__main__.py": b"from . import main\n\nmain()\n",
    f"{DATA}/scripts/rattler-fixture-script": b'#!python\nfrom rattler_fixture import main\n\nmain()\n',
    f"{DATA}/data/share/rattler_fixture/cow.txt": b"(__)\n(oo)\n",
    f"{DIST_INFO}/METADATA": (
        b"Metadata-Version: 2.1\nName: rattler-fixture\nVersion: 1.0.0\nSummary: Fixture package for rattler wheel tests\n"
        b"License: MIT\nRequires-Python: >=3.8\nRequires-Dist: typing-extensions>=4\n"
    ),
    f"{DIST_INFO}/WHEEL": b"Wheel-Version: 1.0\nGenerator: rattler-test-fixture\nRoot-Is-Purelib: true\nTag: py3-none-any\n",
    f"{DIST_INFO}/entry_points.txt": b"[console_scripts]\nrattler-fixture = rattler_fixture:main\n\n[gui_scripts]\nrattler-fixture-gui = rattler_fixture:main\n",
    f"{DIST_INFO}/top_level.txt": b"rattler_fixture\n",
}

def record_line(path, data):
    digest = base64.urlsafe_b64encode(hashlib.sha256(data).digest()).rstrip(b"=").decode()
    return f"{path},sha256={digest},{len(data)}\n"

record = "".join(record_line(p, d) for p, d in files.items()) + f"{DIST_INFO}/RECORD,,\n"
files[f"{DIST_INFO}/RECORD"] = record.encode()

out = sys.argv[1]
with zipfile.ZipFile(out, "w", zipfile.ZIP_DEFLATED) as zf:
    for path, data in files.items():
        info = zipfile.ZipInfo(path, date_time=STAMP)
        info.compress_type = zipfile.ZIP_DEFLATED
        info.external_attr = (0o755 if "/scripts/" in path else 0o644) << 16
        zf.writestr(info, data)
print(out, hashlib.sha256(open(out, "rb").read()).hexdigest())
