#[cfg(any(
    target_os = "macos",
    all(target_os = "linux", target_arch = "x86_64"),
    all(target_os = "linux", target_arch = "aarch64"),
))]
mod tests_impl;

#[cfg(any(
    target_os = "macos",
    all(target_os = "linux", target_arch = "x86_64"),
    all(target_os = "linux", target_arch = "aarch64"),
))]
fn main() {
    rattler_sandbox::init_sandbox();
    let args = libtest_mimic::Arguments::from_args();
    let tests = tests_impl::tests();
    libtest_mimic::run(&args, tests).exit();
}

#[cfg(not(any(
    target_os = "macos",
    all(target_os = "linux", target_arch = "x86_64"),
    all(target_os = "linux", target_arch = "aarch64"),
)))]
fn main() {
    eprintln!("This platform is not supported by the sandbox");
    std::process::exit(0);
}
