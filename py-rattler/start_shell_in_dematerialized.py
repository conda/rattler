#!/usr/bin/env python3
import argparse
import ctypes
import ctypes.util
import os
import subprocess
import sys
from pathlib import Path
from tempfile import TemporaryDirectory

from rattler.shell import Shell, activate, ActivationVariables

libc = ctypes.CDLL(ctypes.util.find_library('c'), use_errno=True)
libc.mount.argtypes = (ctypes.c_char_p, ctypes.c_char_p, ctypes.c_char_p, ctypes.c_ulong, ctypes.c_char_p)


def mount(source, target, fs, options=''):
    ret = libc.mount(source.encode(), str(target).encode(), fs.encode(), 0, options.encode())
    if ret < 0:
        errno = ctypes.get_errno()
        raise OSError(errno, f"Error mounting {source} ({fs}) on {target} with options '{options}': {os.strerror(errno)}")


def build_overlay(temp_dir: Path, underlays: list[str], i: int = 0) -> tuple[str, dict[Path, str]]:
    page_size = os.sysconf("SC_PAGE_SIZE")
    mount_options: dict[Path, str] = {}
    underlays = underlays.copy()
    while underlays:
        options = "lowerdir="
        while underlays and len(options) + len(str(underlays[0])) < page_size:
            options += f"{underlays.pop(0)}:"
        options = options[:-1]
        mount_options[temp_dir / str(i)] = options
        i += 1
    if len(mount_options) == 1:
        lowerdirs = mount_options.pop(temp_dir / str(i - 1))
    else:
        lowerdirs, extra_mounts = build_overlay(temp_dir, list(mount_options), i)
        mount_options |= extra_mounts
    return lowerdirs, mount_options


def mount_overlay(target: Path, options: str, overlay_method: str):
    match overlay_method:
        case "native":
            mount("overlay", target, "overlay", options)
        case "overlayfs-fuse":
            subprocess.run(["fuse-overlayfs", "-o", options, target])
        case _:
            raise NotImplementedError(f"Unsupported overlay method: {overlay_method}")


def main(prefix: Path, overlay_path: Path | None, overlay_method: str):
    underlays_path = prefix / ".dematerialized" / "underlays"
    if not underlays_path.exists():
        raise RuntimeError("This is not a dematerialized environment!")
    cache_underlays = [path.resolve(strict=True) for path in underlays_path.iterdir() if path.is_symlink()]

    if overlay_path:
        overlay_path.mkdir(parents=True, exist_ok=True)
        work_path = overlay_path.with_suffix(".work")
        work_path.mkdir(parents=True, exist_ok=True)

    with TemporaryDirectory() as tempdir:
        lowerdirs, mount_options = build_overlay(Path(tempdir), cache_underlays)

        # Unshare mount and user namespaces, and map current user to root
        uid = os.getuid()
        gid = os.getgid()
        os.unshare(os.CLONE_NEWNS | os.CLONE_NEWUSER)
        Path("/proc/self/uid_map").write_text(f"0 {uid} 1")
        Path("/proc/self/setgroups").write_text("deny")
        Path("/proc/self/gid_map").write_text(f"0 {gid} 1")

        # Make the root filesystem private
        MS_REC = 0x4000
        MS_PRIVATE = 0x40000
        libc.mount("none".encode(), "/".encode(), None, MS_REC | MS_PRIVATE, None)

        # It's only possible to pass up to PAGE_SIZE bytes of options to the
        # mount syscall. If the options are too long, we need to mount the
        # intermediate underlays first.
        for path, options in mount_options.items():
            print("Mounting intermediate underlay at path:", path)
            path.mkdir(parents=True, exist_ok=False)
            mount_overlay(path, options, overlay_method)

        # Mount the final overlayfs instance
        options = lowerdirs.replace('=', f'={prefix}:', 1)
        if overlay_path:
            options += f",upperdir={overlay_path},workdir={work_path}"
        if len(options) > os.sysconf("SC_PAGE_SIZE"):
            raise RuntimeError("Overlay options are too long!")
        mount_overlay(prefix, options, overlay_method)

        # Activate the environment
        activation_script = Path(tempdir) / "activate"
        actvars = ActivationVariables(None, sys.path)
        a = activate(prefix, actvars, Shell.bash)
        activation_script.write_text(
            f"unset BASH_ENV\n{a.script}"
        )

        # TODO: We can't clean up the temporary directory if we exec into bash
        # because the process will be replaced. Maybe we can set an atexit hook
        # to clean up the temporary directory during the activation process?
        cmd = ["bash", "--norc", "--noprofile", "-c", "exec bash --norc --noprofile"]
        os.execvpe("bash", cmd, os.environ | {"BASH_ENV": str(activation_script)})


if __name__ == '__main__':
    parser = argparse.ArgumentParser()
    group = parser.add_mutually_exclusive_group(required=True)
    group.add_argument("--overlay-dir", type=Path, help="Path to overlay directory")
    group.add_argument("--without-writable-overlay", action="store_true", help="Disable writable overlay")
    parser.add_argument("--overlay-method", choices=["native", "overlayfs-fuse"], help="Overlay method to use", required=True)
    parser.add_argument("prefix", type=Path)
    args = parser.parse_args()

    main(prefix=args.prefix, overlay_path=args.overlay_dir, overlay_method=args.overlay_method)
