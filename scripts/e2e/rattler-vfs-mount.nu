#!/usr/bin/env nu

# E2E mount test for rattler_vfs
#
# Usage (Linux):
#   TRANSPORT=fuse OVERLAY=false pixi run -e rattler-vfs e2e-rattler-vfs   # FUSE read-only
#   TRANSPORT=fuse OVERLAY=true  pixi run -e rattler-vfs e2e-rattler-vfs   # FUSE writable
#   TRANSPORT=nfs  OVERLAY=false pixi run -e rattler-vfs e2e-rattler-vfs   # NFS read-only
#   TRANSPORT=nfs  OVERLAY=true  pixi run -e rattler-vfs e2e-rattler-vfs   # NFS writable
# Usage (macOS):
#   TRANSPORT=nfs  OVERLAY=false pixi run -e rattler-vfs e2e-rattler-vfs   # NFS read-only
#   TRANSPORT=nfs  OVERLAY=true  pixi run -e rattler-vfs e2e-rattler-vfs   # NFS writable
# Usage (Windows):
#   TRANSPORT=projfs pixi run -e rattler-vfs e2e-rattler-vfs              # ProjFS (always writable)
#
# Env vars:
#   TRANSPORT  — "fuse", "nfs", or "projfs"
#   OVERLAY    — "true" for writable overlay, anything else for read-only
#                (ignored for ProjFS, which is always writable)

let transport = ($env.TRANSPORT? | default "nfs")
# ProjFS is always writable — it writes directly to the virtualization root.
let use_overlay = if $transport == "projfs" { true } else { ($env.OVERLAY? | default "false") == "true" }

let tmp = ($env.RUNNER_TEMP? | default "/tmp")
let overlay_suffix = if $use_overlay { "-rw" } else { "-ro" }
let mount_point = $"($tmp)/rattler-vfs-($transport)($overlay_suffix)"
let overlay_dir = $"($tmp)/rattler-vfs-overlay-($transport)($overlay_suffix)"
let log_file = $"($tmp)/rattler-vfs-($transport)($overlay_suffix).log"
let fixture_lock = "test-data/rattler-vfs/pixi.lock"

# Python location differs by platform
let python_path = if $nu.os-info.name == "windows" {
    $"($mount_point)/python.exe"
} else {
    $"($mount_point)/bin/python3"
}

# Timing results
mut results: list<record<desc: string, ok: bool, elapsed: duration>> = []

def run [desc: string, cmd: closure] {
    let start = (date now)
    print $"== ($desc)"
    try { do $cmd } catch { }
    let code = ($env.LAST_EXIT_CODE? | default 0)
    let elapsed = ((date now) - $start)
    if $code != 0 {
        print $"FAIL: ($desc) \(exit=($code), ($elapsed))"
        { desc: $desc, ok: false, elapsed: $elapsed }
    } else {
        print $"PASS: ($desc) \(($elapsed))"
        { desc: $desc, ok: true, elapsed: $elapsed }
    }
}

# Helper: run a critical test — bail immediately on failure
def run_critical [desc: string, cmd: closure] {
    let result = (run $desc $cmd)
    if not $result.ok {
        print $"\nFATAL: critical test '($desc)' failed — aborting"
        exit 1
    }
    $result
}

# Helper: expect a command to FAIL (for read-only tests)
def expect_fail [desc: string, cmd: closure] {
    let start = (date now)
    print $"== ($desc)"
    let failed = try { do $cmd; false } catch { true }
    let elapsed = ((date now) - $start)
    if $failed {
        print $"PASS: ($desc) \(($elapsed))"
        { desc: $desc, ok: true, elapsed: $elapsed }
    } else {
        print $"FAIL: ($desc) — operation should have been rejected"
        { desc: $desc, ok: false, elapsed: $elapsed }
    }
}

# ---------------------------------------------------------------------------
# Helper: start `rattler mount` and wait for mount
# ---------------------------------------------------------------------------
def start_and_wait [lock: string, mount: string, transport: string, overlay_args: list<string>, log: string, python: string] {
    print $"== Starting rattler mount... \(transport=($transport))"
    # Wrap the external invocation in try/catch so that when the process is
    # killed (e.g. SIGKILL in the stale mount test), nushell does not surface
    # `terminated by signal` as a fatal error from the spawned job.
    let fs_job = job spawn {
        try {
            ^rattler mount $lock $mount --transport $transport ...$overlay_args out+err> $log
        } catch { }
    }

    print $"== Waiting for mount... \(checking ($python))"
    # On unix, use external `test -e` wrapped in `timeout` (provided by the
    # rattler-vfs-test pixi env's coreutils) instead of nushell's `path exists`:
    # the latter calls `fs::metadata`, which blocks uninterruptibly on a stale
    # NFS mount left behind by a prior test's incomplete umount. On Windows
    # there's no analogous hang (ProjFS), so plain `path exists` is fine.
    let is_windows = ($nu.os-info.name == "windows")
    if not (seq 0 59 | any {|_|
        let exists = if $is_windows {
            ($python | path exists)
        } else {
            try { ^timeout 1 test -e $python; true } catch { false }
        }
        if $exists {
            true
        } else {
            # Check if the process died — no point waiting 120s for a dead process
            let alive = (job list | where id == $fs_job | length) > 0
            if not $alive {
                print "== rattler mount exited before mount was ready:"
                try { open $log | lines | each { |l| print $"  ($l)" } } catch { }
                error make {msg: "rattler mount exited before mount was ready (see log above)"}
            }
            sleep 2sec
            false
        }
    }) {
        print "== Mount failed to become ready after 120 seconds. Log tail:"
        try { open $log | lines | last 20 | each { |l| print $l } } catch { }
        error make {msg: "Mount failed to become ready within 120 seconds"}
    }
    print "== Mount is ready"
    $fs_job
}

# Helper: stop `rattler mount` and clean up mount
def stop_and_cleanup [fs_job: int, transport: string, mount: string] {
    # Kill the process first — triggers unmount via MountHandle::drop
    try { job kill $fs_job } catch { }
    sleep 1sec  # allow time for drop/unmount

    # Safety-net unmount in case the process didn't clean up
    if $transport == "fuse" {
        try { ^fusermount3 -u $mount } catch {
            try { ^umount $mount } catch { }
        }
    } else if $transport == "nfs" {
        # macOS: user-level umount works first; if the mount is still
        # attached (umount returned 0 but cleanup is async, or umount
        # failed silently), retry with diskutil's force-detach which
        # guarantees the mount is gone before the next remount tries the
        # same path.
        # Linux: needs root, AND the userspace helper `umount.nfs` refuses
        # to detach mounts whose loopback server has died. Bypass it with
        # `-i` (skip helper) and `-l` (lazy detach) so the kernel always
        # succeeds. Otherwise stale mounts accumulate across tests and the
        # loopback NFS subsystem eventually wedges.
        if $nu.os-info.name == "linux" {
            try { ^timeout 10 sudo umount -i -f -l $mount } catch { }
        } else {
            try { ^timeout 10 umount -f $mount } catch {
                try { ^timeout 10 sudo umount -f $mount } catch { }
            }
            # Verify and force-detach if still mounted (macOS).
            let still_mounted = (try {
                let out = (^timeout 5 mount | complete)
                ($out.stdout | str contains $mount)
            } catch { false })
            if $still_mounted {
                try { ^timeout 10 diskutil unmount force $mount } catch { }
            }
        }
    }
    # ProjFS: no explicit unmount needed — PrjStopVirtualizing is called on drop.
    # Allow extra time for ProjFS to release file handles after stop.
    if $transport == "projfs" {
        sleep 2sec
    }
}

# ---------------------------------------------------------------------------
# Setup
# ---------------------------------------------------------------------------
print $"== Configuration: transport=($transport) overlay=($use_overlay)"
mkdir $mount_point
if $use_overlay and $transport != "projfs" {
    mkdir $overlay_dir
}

let overlay_args = if $transport == "projfs" {
    ["--overlay", $mount_point]  # ProjFS: always writable, overlay == mount point
} else if $use_overlay {
    ["--overlay", $overlay_dir]
} else {
    []
}

# ---------------------------------------------------------------------------
# Start mount
# ---------------------------------------------------------------------------
let fs_job = (start_and_wait $fixture_lock $mount_point $transport $overlay_args $log_file $python_path)

# ---------------------------------------------------------------------------
# Read-only tests (always run)
# ---------------------------------------------------------------------------

# On Windows, conda environments need Library/bin (MKL, OpenBLAS, etc.) on
# PATH for DLL loading.  Mimic the activation entries that rattler_shell's
# prefix_path_entries() adds.
if $nu.os-info.name == "windows" {
    $env.PATH = ([
        $mount_point,
        ($mount_point | path join "Library" "mingw-w64" "bin"),
        ($mount_point | path join "Library" "usr" "bin"),
        ($mount_point | path join "Library" "bin"),
        ($mount_point | path join "Scripts"),
        ($mount_point | path join "bin"),
    ] | append $env.PATH)
}

# Directory traversal (skip on Windows — find not available)
if $nu.os-info.name != "windows" {
    $results = ($results | append (run_critical "find all files" {
        let count = (^find $mount_point -type f | lines | length)
        print $"  Found ($count) files"
        if $count < 10 {
            error make {msg: $"Expected at least 10 files, found ($count)"}
        }
    }))
}

# Python import
$results = ($results | append (run_critical "import numpy" {
    ^$python_path -c "import numpy; print(f'numpy {numpy.__version__}')"
}))

# Read-only enforcement (only when not using overlay).
# Skip on ProjFS: there is no pre-creation notification, and NTFS ACLs block
# ProjFS placeholder creation too, so new file/dir creation can't be blocked.
if not $use_overlay and $transport != "projfs" {
    $results = ($results | append (expect_fail "write file rejected on read-only mount" {
        "test" | save $"($mount_point)/should_not_exist.txt"
    }))

    $results = ($results | append (expect_fail "mkdir rejected on read-only mount" {
        mkdir $"($mount_point)/should_not_exist_dir"
    }))
}

# Symlink resolution (skip on Windows — symlinks handled differently)
if $nu.os-info.name != "windows" {
    $results = ($results | append (run "symlink resolution" {
        # Find a symlink in the mount and verify its target exists
        let symlinks = (^find $mount_point -type l -maxdepth 3 | lines | first 5)
        if ($symlinks | length) == 0 {
            error make {msg: "Expected at least one symlink in the mount"}
        }
        for link in $symlinks {
            let target = (^readlink $link)
            print $"  ($link) -> ($target)"
            # Resolve relative to the link's parent directory
            let parent = ($link | path dirname)
            let resolved = if ($target | str starts-with "/") {
                $target
            } else {
                $"($parent)/($target)"
            }
            # The target should exist (either as a file or another symlink)
            if not ($resolved | path exists) {
                error make {msg: $"Symlink target does not exist: ($link) -> ($target) (resolved: ($resolved))"}
            }
        }
        print $"  Verified ($symlinks | length) symlinks"
    }))
}

# ---------------------------------------------------------------------------
# Overlay tests (only when writable)
# ---------------------------------------------------------------------------
if $use_overlay {
    $results = ($results | append (run "write and read file" {
        "hello from rattler-vfs" | save $"($mount_point)/test_write.txt"
        let content = (open $"($mount_point)/test_write.txt")
        if $content != "hello from rattler-vfs" {
            error make {msg: $"Content mismatch: ($content)"}
        }
        rm $"($mount_point)/test_write.txt"
    }))

    # Verify overlay stores data in the right place (FUSE/NFS only — ProjFS writes in-place)
    if $transport != "projfs" {
        $results = ($results | append (run "overlay data placement" {
            "placement test" | save $"($mount_point)/overlay_check.txt"
            if not ($"($overlay_dir)/overlay_check.txt" | path exists) {
                error make {msg: "Write did not appear in overlay directory"}
            }
            rm $"($mount_point)/overlay_check.txt"
        }))
    }

    $results = ($results | append (run "mkdir and rename" {
        mkdir $"($mount_point)/test_dir"
        "test" | save $"($mount_point)/test_dir/a.txt"
        mv $"($mount_point)/test_dir/a.txt" $"($mount_point)/test_dir/b.txt"
        let content = (open $"($mount_point)/test_dir/b.txt")
        if $content != "test" {
            error make {msg: $"Content mismatch after rename: ($content)"}
        }
    }))

    # pip install scipy
    $results = ($results | append (run "pip install scipy" {
        ^$python_path -m pip install --prefix $mount_point --no-build-isolation scipy
    }))

    $results = ($results | append (run "import scipy" {
        ^$python_path -c "import scipy; print(f'scipy {scipy.__version__}')"
    }))

    # Write a persistence marker before the destructive uninstall tests
    "persist_marker" | save $"($mount_point)/persist_test.txt"

    # pip uninstall scipy (pip-installed package)
    $results = ($results | append (run "pip uninstall scipy" {
        ^$python_path -m pip uninstall -y scipy
    }))

    $results = ($results | append (expect_fail "import scipy after uninstall" {
        ^$python_path -c "import scipy"
    }))

    # pip uninstall pytest (conda-installed package — exercises overlay whiteouts
    # for lower-layer file deletion).
    # Skip on ProjFS: the base layer (conda packages) is immutable and defined
    # by the lock file.  Uninstalling a base-layer package should be done by
    # modifying the lock file and remounting, not via pip.  ProjFS placeholder
    # directories also cannot be renamed (ERROR_NOT_SUPPORTED due to reparse
    # tags), which breaks pip's uninstall stash step.  Pip can still
    # install/uninstall in the overlay (as tested by the scipy tests above).
    if $transport != "projfs" {
        $results = ($results | append (run "pip uninstall pytest" {
            ^$python_path -m pip uninstall -y pytest
        }))

        $results = ($results | append (expect_fail "import pytest after uninstall" {
            ^$python_path -c "import pytest"
        }))
    }

    # Large file COW materialization
    $results = ($results | append (run "large file COW materialization" {
        let large_file = $"($mount_point)/large_test.bin"
        # Write a 10 MB file through the overlay.
        # Use a Python raw string for the path so Windows backslashes
        # (e.g. `D:\a\...` → `\a` = BEL) are not interpreted as escapes.
        ^$python_path -c $"
import os
data = b'A' * (10 * 1024 * 1024)
with open\(r'($large_file)', 'wb') as f:
    f.write\(data)
"
        # Verify the file size
        let size = (ls $large_file | get size | first)
        print $"  Wrote ($size) file"
        if $size < 10MB {
            error make {msg: $"Expected >= 10 MB, got ($size)"}
        }
        # Read back and verify content
        ^$python_path -c $"
with open\(r'($large_file)', 'rb') as f:
    data = f.read\()
assert len\(data) == 10 * 1024 * 1024, f'Size mismatch: {len\(data)}'
assert data == b'A' * len\(data), 'Content mismatch'
"
        # Verify it landed in the overlay directory (FUSE/NFS only)
        if $transport != "projfs" {
            if not ($"($overlay_dir)/large_test.bin" | path exists) {
                error make {msg: "Large file did not appear in overlay directory"}
            }
        }
        rm $large_file
    }))

    # --- Persistence: unmount and remount ---
    print "\n== Persistence test: unmount and remount"

    stop_and_cleanup $fs_job $transport $mount_point

    # Remount with the same overlay (use a separate log file to preserve earlier logs)
    let log_file2 = $"($tmp)/rattler-vfs-($transport)($overlay_suffix)-remount.log"
    let fs_job2 = (start_and_wait $fixture_lock $mount_point $transport $overlay_args $log_file2 $python_path)

    $results = ($results | append (run "persisted file survives remount" {
        let content = (open $"($mount_point)/persist_test.txt")
        if $content != "persist_marker" {
            error make {msg: $"Persisted file content mismatch: ($content)"}
        }
    }))

    $results = ($results | append (run "renamed file survives remount" {
        let content = (open $"($mount_point)/test_dir/b.txt")
        if $content != "test" {
            error make {msg: $"Renamed file content mismatch: ($content)"}
        }
    }))

    stop_and_cleanup $fs_job2 $transport $mount_point

    # --- Benign lock-file edit preserves overlay ---
    #
    # compute_env_hash hashes the resolved package list, not raw lock-file
    # bytes, so a trailing YAML comment must NOT change the env hash.
    # Regression test for the per-package hash behavior.
    print "\n== Benign lock-file edit test"

    let edited_lock = $"($tmp)/rattler-vfs-edited.lock"
    open $fixture_lock | $"($in)\n# trailing comment" | save -f $edited_lock

    let log_file3 = $"($tmp)/rattler-vfs-($transport)($overlay_suffix)-edited.log"
    let fs_job3 = (start_and_wait $edited_lock $mount_point $transport $overlay_args $log_file3 $python_path)

    $results = ($results | append (run "overlay survives benign lock-file edit" {
        let content = (open $"($mount_point)/persist_test.txt")
        if $content != "persist_marker" {
            error make {msg: $"Persisted file lost after lock-file edit: ($content)"}
        }
    }))

    stop_and_cleanup $fs_job3 $transport $mount_point

    # --- Env-hash mismatch rejects mount ---
    #
    # After a genuine package change, the overlay's stored env hash won't match.
    # The mount should refuse to start (not silently wipe the overlay).
    print "\n== Env-hash mismatch test"

    let modified_lock = $"($tmp)/rattler-vfs-modified.lock"
    # Change the first sha256 to produce a different env hash
    open $fixture_lock | str replace --regex 'sha256: [0-9a-f]{64}' 'sha256: 0000000000000000000000000000000000000000000000000000000000000000' | save -f $modified_lock

    $results = ($results | append (expect_fail "env-hash mismatch rejects stale overlay" {
        let log_mismatch = $"($tmp)/rattler-vfs-($transport)($overlay_suffix)-mismatch.log"
        # Bounded with `timeout` on unix (provided by coreutils in the pixi
        # env): rattler should refuse and exit, but if it hangs (e.g. blocking
        # on cache lock acquisition), `expect_fail` never returns and the
        # whole step times out at 15 min. Windows: no `timeout` binary, and
        # ProjFS doesn't have NFS-style hangs anyway.
        if $nu.os-info.name == "windows" {
            ^rattler mount $modified_lock $mount_point --transport $transport ...$overlay_args out+err> $log_mismatch
        } else {
            ^timeout 60 rattler mount $modified_lock $mount_point --transport $transport ...$overlay_args out+err> $log_mismatch
        }
    }))

    # --- Transport mismatch rejects mount ---
    #
    # Overlay records which transport created it. Mounting with a different
    # transport must fail rather than silently corrupting state.
    # Only testable on Linux where both FUSE and NFS are available.
    let other_transport = if $transport == "fuse" { "nfs" } else if $transport == "nfs" and $nu.os-info.name == "linux" { "fuse" } else { "" }
    if $other_transport != "" {
        print "\n== Transport mismatch test"

        $results = ($results | append (expect_fail "transport mismatch rejects mount" {
            let log_transport = $"($tmp)/rattler-vfs-($transport)($overlay_suffix)-transport.log"
            ^timeout 60 rattler mount $fixture_lock $mount_point --transport $other_transport ...$overlay_args out+err> $log_transport
        }))
    }
}

# ---------------------------------------------------------------------------
# Negative tests (no mount needed)
# ---------------------------------------------------------------------------

# Network failure: mount with an unreachable package URL.
# Only run on non-Windows (lock file packages differ per platform).
if $nu.os-info.name != "windows" {
    print "\n== Network failure test"

    let bad_lock = $"($tmp)/rattler-vfs-bad-url.lock"
    open $fixture_lock | str replace --all "https://prefix.dev" "http://127.0.0.1:1" | save -f $bad_lock

    let bad_mount = $"($tmp)/rattler-vfs-bad-url-mount"
    mkdir $bad_mount
    let bad_log = $"($tmp)/rattler-vfs-bad-url.log"

    # The package cache is content-addressed (keyed by name/version/build +
    # sha256, not URL), so the packages downloaded by the earlier tests would
    # satisfy this mount without touching the network — and the mount would
    # *succeed* and block forever waiting for a signal. Point the cache at an
    # empty directory so the unreachable URL is actually exercised.
    let bad_cache = $"($tmp)/rattler-vfs-bad-url-cache"
    mkdir $bad_cache

    # Wrap with `timeout` so a hang in retry/backoff fails the test instead of
    # the whole step (15 min). `timeout` exits 124 on timeout, which still
    # satisfies `expect_fail` (any non-zero exit counts).
    $results = ($results | append (expect_fail "mount fails with unreachable package URL" {
        with-env {RATTLER_CACHE_DIR: $bad_cache} {
            ^timeout 60 rattler mount $bad_lock $bad_mount --transport $transport out+err> $bad_log
        }
    }))

    try { ^timeout 5 rm -rf $bad_mount $bad_cache } catch { }
}

# ---------------------------------------------------------------------------
# Shutdown tests
# ---------------------------------------------------------------------------

# Graceful shutdown with a busy mount (open file handle during kill).
# Skip on Windows — job/signal semantics differ.
if $nu.os-info.name != "windows" {
    print "\n== Graceful shutdown test"

    let shutdown_mount = $"($tmp)/rattler-vfs-shutdown"
    let shutdown_log = $"($tmp)/rattler-vfs-shutdown.log"
    mkdir $shutdown_mount

    let shutdown_python = $"($shutdown_mount)/bin/python3"
    let fs_job_shutdown = (start_and_wait $fixture_lock $shutdown_mount $transport [] $shutdown_log $shutdown_python)

    # Start a background process that holds a file handle open
    let reader_job = job spawn {
        ^$shutdown_python -c "import time; f = open(__file__, 'rb'); time.sleep(300)"
    }
    sleep 1sec  # let the reader open the file

    # Kill the mount process (sends SIGTERM → triggers graceful shutdown)
    try { job kill $fs_job_shutdown } catch { }

    # Wait up to 10 seconds for process to exit
    let exited = (seq 0 9 | any {|_|
        let alive = (job list | where id == $fs_job_shutdown | length) > 0
        if not $alive { true } else { sleep 1sec; false }
    })

    try { job kill $reader_job } catch { }

    $results = ($results | append (if $exited {
        print "PASS: graceful shutdown with busy mount (mount exited)"
        { desc: "graceful shutdown with busy mount", ok: true, elapsed: 0sec }
    } else {
        print "FAIL: graceful shutdown with busy mount — process did not exit within 10s"
        { desc: "graceful shutdown with busy mount", ok: false, elapsed: 10sec }
    }))

    stop_and_cleanup $fs_job_shutdown $transport $shutdown_mount
    # Wrap with `timeout`: if `stop_and_cleanup`'s umount didn't actually
    # detach (e.g. NFS stuck), `rm -rf` would `readdir` into the still-mounted
    # path and block forever (a blocked syscall isn't catchable by `try`).
    try { ^timeout 5 rm -rf $shutdown_mount } catch { }
}

# NFS stale mount: SIGKILL the process (no cleanup), verify force-unmount works.
# NFS-only — FUSE auto-unmounts when the FUSE fd is closed.
#
# Linux is excluded: GHA Linux runners' NFS client wedges after several
# mount/umount cycles in this script (umount.nfs returns EPERM under sudo
# even though `umount -i` should bypass it), and a subsequent mount on a
# fresh path then hangs uninterruptibly. The same script runs cleanly on
# Linux outside GHA (locally and on self-hosted runners with privileged
# NFS support) — this is a runner-image limitation, not a code issue.
#
# macOS NFS already exercises this code path successfully — that's the
# platform whose unmount-on-stale behavior we actually ship to users.
# Moving Linux NFS coverage to a self-hosted or container runner with
# proper privilege isolation is tracked as out-of-scope follow-up work.
if $transport == "nfs" and $nu.os-info.name != "linux" {
    print "\n== NFS stale mount test"

    let stale_mount = $"($tmp)/rattler-vfs-stale"
    let stale_log = $"($tmp)/rattler-vfs-stale.log"
    mkdir $stale_mount

    let stale_python = $"($stale_mount)/bin/python3"
    let fs_job_stale = (start_and_wait $fixture_lock $stale_mount $transport [] $stale_log $stale_python)

    # SIGKILL — bypasses graceful shutdown, NFS server dies without unmounting.
    # Use `pgrep -f` instead of `ps -l | where command =~ ...`: pgrep is
    # available on macOS and Linux runners and walks /proc directly without
    # nushell's sysinfo wrapper (which surfaces error values for the `command`
    # column on some runners). Fall back to 0 → kill no-op if not found, so
    # the test fails cleanly instead of hanging.
    let pid_str = (try { ^pgrep -f "rattler.*stale" | lines | first } catch { "" })
    if ($pid_str | is-empty) {
        print "WARN: could not find rattler stale process via pgrep -f"
    } else {
        try { ^kill -9 ($pid_str | into int) } catch { }
    }
    sleep 2sec

    # Mount should be stale — reads should fail.  Wrap with `timeout` so a
    # hanging NFS read (Linux soft-mount default is hard, and reads block
    # indefinitely on a dead server) fails the test fast instead of hitting
    # the step's 15-min wall-clock.
    let stale_read_failed = try {
        ^timeout 5 cat $"($stale_mount)/bin/python3" out+err> /dev/null
        false
    } catch { true }

    # Force unmount should clean up. Linux: `-i` bypasses umount.nfs (which
    # refuses on a stale mount), `-l` lazy-detaches. macOS: `-f` is enough.
    # `timeout` guards against macOS umount blocking on a dead NFS server.
    let force_unmount_ok = try {
        if $nu.os-info.name == "linux" {
            ^timeout 10 sudo umount -i -f -l $stale_mount
        } else {
            ^timeout 10 umount -f $stale_mount
        }
        true
    } catch { false }

    # Make sure the spawned job is reaped — nushell will block on script exit
    # waiting for live jobs, which would mask the test failure as a wall-clock
    # timeout.
    try { job kill $fs_job_stale } catch { }

    # The contract this test asserts is rattler's: when the mount process
    # dies, reads on the mount must fail (i.e. the mount is observably stale).
    # `force_unmount_ok` is logged as a side-channel but doesn't gate the test
    # — that's a property of the kernel NFS client + admin privileges (CI
    # runners have varying behavior here, e.g. Linux's umount.nfs returning
    # `Operation not permitted` even under sudo with `-l`).
    $results = ($results | append (if $stale_read_failed {
        print $"PASS: NFS stale mount detected \(force_unmount=($force_unmount_ok))"
        { desc: "NFS stale mount detected", ok: true, elapsed: 0sec }
    } else {
        print $"FAIL: NFS stale mount — read_failed=($stale_read_failed) force_unmount=($force_unmount_ok)"
        { desc: "NFS stale mount detected", ok: false, elapsed: 0sec }
    }))

    # If the mount is still attached (force_unmount_ok=false), skip the rm
    # — `rm -rf` would `readdir` into the stale NFS mount and block forever
    # (a blocked syscall isn't catchable by `try`). Best-effort otherwise.
    if $force_unmount_ok {
        try { ^timeout 5 rm -rf $stale_mount } catch { }
    } else {
        print $"NOTE: leaving stale mount at ($stale_mount) for the runner-level cleanup"
    }
}

# ---------------------------------------------------------------------------
# Summary
# ---------------------------------------------------------------------------
let all_ok = ($results | all { |r| $r.ok })

print "\n== Results:"
for r in $results {
    let status = if $r.ok { "PASS" } else { "FAIL" }
    print $"  ($status) ($r.desc) \(($r.elapsed))"
}

# Write to GitHub Actions job summary if available
if ($env.GITHUB_STEP_SUMMARY? | is-not-empty) {
    let header = $"## rattler-vfs mount test \(($transport), overlay=($use_overlay))\n\n| Test | Status | Time |\n|------|--------|------|\n"
    let rows = ($results | each { |r|
        let status = if $r.ok { "PASS" } else { "FAIL" }
        $"| ($r.desc) | ($status) | ($r.elapsed) |"
    } | str join "\n")
    $"($header)($rows)\n" | save --append $env.GITHUB_STEP_SUMMARY
}

# ---------------------------------------------------------------------------
# Final cleanup
# ---------------------------------------------------------------------------
print "\n== Cleaning up..."

# For non-overlay tests or if overlay tests already cleaned up
if not $use_overlay {
    stop_and_cleanup $fs_job $transport $mount_point
}

# Clean up directories. Wrap rm with `timeout` for the same reason as the
# stale-mount test: a still-attached/stale NFS mount makes `readdir` block
# forever, which `try` cannot interrupt.
if $use_overlay and $transport != "projfs" {
    try { ^timeout 5 rm -rf $overlay_dir } catch { }
}
# ProjFS may leave behind tombstones/placeholders that can't be removed
# immediately after stopping virtualization — ignore cleanup errors.
try { ^timeout 5 rm -rf $mount_point } catch { }

if not $all_ok {
    print "\n== rattler mount log tail:"
    try { open $log_file | lines | last 30 | each { |l| print $l } } catch { }
    exit 1
}
