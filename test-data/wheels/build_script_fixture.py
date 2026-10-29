"""Generate the minimal wheel used by the raw-script installation regression."""

import base64
import csv
import hashlib
import io
from pathlib import Path
import zipfile

files = {
    "script_probe-1.0.dist-info/METADATA": b"Metadata-Version: 2.1\nName: script-probe\nVersion: 1.0\n",
    "script_probe-1.0.dist-info/WHEEL": b"Wheel-Version: 1.0\nRoot-Is-Purelib: true\nTag: py3-none-any\n",
    "script_probe-1.0.data/scripts/script-probe": b'#!python\nprint("hello")\n',
    "script_probe-1.0.data/scripts/script-probe-gui": b'#!pythonw\r\nprint("gui")\n',
    "script_probe-1.0.data/scripts/plain-script": b'#!/bin/sh\necho hello\n',
}
record = "script_probe-1.0.dist-info/RECORD"
rows = io.StringIO()
writer = csv.writer(rows, lineterminator="\n")
for name, contents in files.items():
    digest = base64.urlsafe_b64encode(hashlib.sha256(contents).digest()).decode().rstrip("=")
    writer.writerow([name, f"sha256={digest}", len(contents)])
writer.writerow([record, "", ""])
files[record] = rows.getvalue().encode()

output = Path(__file__).with_name("script_probe-1.0-py3-none-any.whl")
with zipfile.ZipFile(output, "w") as wheel:
    for name, contents in files.items():
        entry = zipfile.ZipInfo(name)
        entry.create_system = 3
        mode = 0o100755 if ".data/scripts/" in name else 0o100644
        entry.external_attr = mode << 16
        wheel.writestr(entry, contents)
