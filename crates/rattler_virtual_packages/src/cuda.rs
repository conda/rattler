use libloading::Symbol;
use once_cell::sync::OnceCell;
use rattler_conda_types::Version;
use std::{
    mem::MaybeUninit,
    os::raw::{c_int, c_uint, c_ulong},
    str::FromStr,
};

/// Returns the maximum Cuda version available on the current platform.
pub fn cuda_version() -> Option<Version> {
    static DETECTED_CUDA_VERSION: OnceCell<Option<Version>> = OnceCell::new();
    DETECTED_CUDA_VERSION
        .get_or_init(detect_cuda_version)
        .clone()
}

/// Attempts to detect the version of CUDA present in the current operating system.
pub fn detect_cuda_version() -> Option<Version> {
    // Try to open the library
    let cuda_library = cuda_library_paths()
        .iter()
        .find_map(|path| unsafe { libloading::Library::new(*path).ok() })?;

    // Get entry points from the library
    let cu_init: Symbol<unsafe extern "C" fn(c_uint) -> c_ulong> =
        unsafe { cuda_library.get(b"cuInit\0") }.ok()?;
    let cu_driver_get_version: Symbol<unsafe extern "C" fn(*mut c_int) -> c_ulong> =
        unsafe { cuda_library.get(b"cuDriverGetVersion\0") }.ok()?;

    // Initialize the CUDA library
    if unsafe { cu_init(0) } != 0 {
        return None;
    }

    // Get the version from the library
    let mut version_int = MaybeUninit::uninit();
    if unsafe { cu_driver_get_version(version_int.as_mut_ptr()) != 0 } {
        return None;
    }
    let version = unsafe { version_int.assume_init() };

    // Convert the version integer to a version string
    Version::from_str(&format!("{}.{}", version / 1000, (version % 1000) / 10)).ok()
}

/// Returns platform specific set of search paths for the CUDA library.
///
/// On Windows and Linux, the CUDA library is installed by the NVIDIA driver package, and is
/// typically found in the standard library path, rather than with the CUDA SDK (which is optional
/// for running CUDA apps).
///
/// On macOS, the CUDA library is only installed with the CUDA SDK, and might not be in the library
/// path.
fn cuda_library_paths() -> &'static [&'static str] {
    #[cfg(target_os = "macos")]
    static FILENAMES: &[&str] = &[
        "libcuda.dylib", // Check library path first
        "/usr/local/cuda/lib/libcuda.dylib",
    ];
    #[cfg(target_os = "linux")]
    static FILENAMES: &[&str] = &[
        "libcuda.so",                           // Check library path first
        "/usr/lib64/nvidia/libcuda.so",         // RHEL/Centos/Fedora
        "/usr/lib/x86_64-linux-gnu/libcuda.so", // Ubuntu
        "/usr/lib/wsl/lib/libcuda.so",          // WSL (workaround)
    ];
    #[cfg(windows)]
    static FILENAMES: &[&str] = &["nvcuda.dll"];
    #[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
    compile_error!("unsupported target os");
    FILENAMES
}

#[cfg(test)]
mod test {
    use super::detect_cuda_version;

    #[test]
    pub fn doesnt_crash() {
        let version = detect_cuda_version();
        println!("{:?}", version);
    }
}
