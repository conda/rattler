fn main() {
    // Delay-load ProjectedFSLib.dll for the `rattler` binary itself.
    //
    // `rattler_vfs`'s own build.rs sets `/DELAYLOAD` via `rustc-link-arg`, but
    // that flag only applies to the artifacts Cargo builds *for that crate* —
    // it does not propagate to a downstream binary that merely depends on the
    // library. Without a delay-load arg on the final link, `rattler.exe` keeps
    // projectedfslib.dll in its normal import table and the Windows loader
    // kills the process before `main()` on machines where the ProjFS optional
    // feature is not installed — defeating the graceful `ProjFsDllMissing`
    // check in `rattler_vfs::mount`. Emit the arg here (via the `-bins`
    // variant, which targets the binaries this crate produces) so the check is
    // actually reachable.
    //
    // NOTE: any *other* downstream binary embedding `rattler_vfs` (e.g. pixi)
    // must do the same on Windows. A libloading-based dynamic binding of the
    // ProjFS API in `rattler_vfs` would remove that requirement (see #2572).
    if std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default() == "windows"
        && std::env::var("CARGO_CFG_TARGET_ENV").unwrap_or_default() == "msvc"
    {
        println!("cargo::rustc-link-arg-bins=/DELAYLOAD:projectedfslib.dll");
        println!("cargo::rustc-link-lib=delayimp");
    }
}
