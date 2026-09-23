//! Default virtual package versions to use when the actual version cannot be
//! detected from the host system, for example when detecting virtual packages
//! for a platform other than the current one (see
//! [`crate::VirtualPackages::detect_for_platform`]).

use rattler_conda_types::{Platform, Version};

/// The default `glibc` version to use when the version cannot be detected.
///
/// This is the `glibc` version that ships with RHEL 8 and Debian 10, except for
/// platforms that never had a `glibc` that old: `riscv64` support only landed in
/// `glibc` 2.27 upstream, and conda-forge builds for `linux-riscv64` against
/// 2.39, so assuming 2.28 there makes every package look uninstallable.
pub fn default_glibc_version(platform: Platform) -> Version {
    match platform {
        Platform::LinuxRiscv64 => "2.39".parse().unwrap(),
        _ => "2.28".parse().unwrap(),
    }
}

/// The default Linux kernel version to use when the version cannot be
/// detected.
///
/// This is the kernel version that ships with RHEL 8.
pub fn default_linux_version() -> Version {
    "4.18".parse().unwrap()
}

/// The default Windows version to use when the version cannot be detected.
pub fn default_windows_version() -> Version {
    "10.0".parse().unwrap()
}

/// Returns the default macOS version to use when the version cannot be
/// detected, or `None` if the given platform is not a macOS platform.
pub fn default_mac_os_version(platform: Platform) -> Option<Version> {
    match platform {
        Platform::Osx64 | Platform::OsxArm64 => Some("13.0".parse().unwrap()),
        _ => None,
    }
}
