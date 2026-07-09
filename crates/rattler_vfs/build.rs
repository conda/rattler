fn main() {
    // Delay-load ProjectedFSLib.dll so the binary can start even when the
    // Windows Projected File System optional feature is not installed.  Without
    // this, the DLL is in the normal import table and the Windows loader kills
    // the process before main() when the DLL is absent.  With delay-load the
    // DLL is only resolved on the first ProjFS API call, giving us a chance to
    // check availability and return a clear error.
    if std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default() == "windows" {
        println!("cargo::rustc-link-arg=/DELAYLOAD:projectedfslib.dll");
        println!("cargo::rustc-link-lib=delayimp");
    }
}
