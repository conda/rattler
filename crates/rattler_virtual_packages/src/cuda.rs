//! Provides functionality to detect the CUDA version present on the current system.
//!
//! Two methods are provided:
//!
//! * [`detect_cuda_version_via_nvml`]
//! * [`detect_cuda_version_via_libcuda`]
//!
//! Both will detect the current supported CUDA version but the first method has less edge cases.
//! See the function documentation for more information.

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
        .get_or_init(detect_cuda_version_via_nvml)
        .clone()
}

/// Attempts to detect the version of CUDA present in the current operating system by loading the
/// NVIDIA Management Library and querying the CUDA driver version. The method is preferred over
/// [`detect_cuda_version_via_libcuda`] because that method might fail base on environment
/// variables.
///
/// Although the required methods in the runtime are not implemented on much older machines it is
/// considered old enough to be usable for our use case. Since Conda doesnt provide old versions of
/// the CUDA SDK anyway this is considered a non-issue.
pub fn detect_cuda_version_via_nvml() -> Option<Version> {
    // Try to open the library
    let library = nvml_library_paths()
        .iter()
        .find_map(|path| unsafe { libloading::Library::new(*path).ok() })?;

    // Get the initialization function. We first try to get `nvmlInit_v2` but if we can't find that
    // we use the `nvmlInit` function.
    let nvml_init: Symbol<unsafe extern "C" fn() -> c_int> = unsafe {
        library
            .get(b"nvmlInit_v2\0")
            .or_else(|_| library.get(b"nvmlInit\0"))
    }
    .ok()?;

    // Find the shutdown function
    let nvml_shutdown: Symbol<unsafe extern "C" fn() -> c_int> =
        unsafe { library.get(b"nvmlShutdown\0") }.ok()?;

    // Find the `nvmlSystemGetCudaDriverVersion_v2` function. If that function cannot be found, fall
    // back to the `nvmlSystemGetCudaDriverVersion` function instead.
    let nvml_system_get_cuda_driver_version: Symbol<unsafe extern "C" fn(*mut c_int) -> c_int> =
        unsafe {
            library
                .get(b"nvmlSystemGetCudaDriverVersion_v2\0")
                .or_else(|_| library.get(b"nvmlSystemGetCudaDriverVersion\0"))
        }
        .ok()?;

    // Call the initialization function
    if unsafe { nvml_init() } != 0 {
        return None;
    }

    // Get the version
    let mut cuda_driver_version = MaybeUninit::uninit();
    let result = unsafe { nvml_system_get_cuda_driver_version(cuda_driver_version.as_mut_ptr()) };

    // Call the shutdown function (don't care about the result of the function). Whatever happens,
    // after calling `nvmlInit` we have to call `nvmlShutdown`.
    let _ = unsafe { nvml_shutdown() };

    // If the call failed we dont have a version
    if result != 0 {
        return None;
    }

    // We can assume the value is initialized by the `nvmlSystemGetCudaDriverVersion` function.
    let version = unsafe { cuda_driver_version.assume_init() };

    // Convert the version integer to a version string
    Version::from_str(&format!("{}.{}", version / 1000, (version % 1000) / 10)).ok()
}

/// Returns platform specific set of search paths for the CUDA library.
///
/// On Windows and Linux, the nvml library is installed by the NVIDIA driver package, and is
/// typically found in the standard library path, rather than with the CUDA SDK (which is optional
/// for running CUDA apps).
///
/// On macOS, the CUDA library is only installed with the CUDA SDK, and might not be in the library
/// path.
fn nvml_library_paths() -> &'static [&'static str] {
    #[cfg(target_os = "macos")]
    static FILENAMES: &[&str] = &[
        "libnvidia-ml.1.dylib", // Check library path first
        "libnvidia-ml.dylib",
        "/usr/local/cuda/lib/libnvidia-ml.1.dylib",
        "/usr/local/cuda/lib/libnvidia-ml.dylib",
    ];
    #[cfg(target_os = "linux")]
    static FILENAMES: &[&str] = &[
        "libnvidia-ml.so.1", // Check library path first
        "libnvidia-ml.so",
        "/usr/lib64/nvidia/libnvidia-ml.so.1", // RHEL/Centos/Fedora
        "/usr/lib64/nvidia/libnvidia-ml.so",
        "/usr/lib/x86_64-linux-gnu/libnvidia-ml.so.1", // Ubuntu
        "/usr/lib/x86_64-linux-gnu/libnvidia-ml.so",
        "/usr/lib/wsl/lib/libnvidia-ml.so.1", // WSL
        "/usr/lib/wsl/lib/libnvidia-ml.so",
        "/usr/lib/x86_64-linux-gnu/nvidia/current/libcuda.so.1", // Debian
        "/usr/lib/x86_64-linux-gnu/nvidia/current/libcuda.so",
    ];
    #[cfg(windows)]
    static FILENAMES: &[&str] = &["nvml.dll"];
    #[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
    compile_error!("unsupported target os");
    FILENAMES
}

/// Attempts to detect the version of CUDA present in the current operating system by loading the
/// cuda runtime library and querying the CUDA driver version.
///
/// The behavior of functions from `libcuda` depend on the environment variable
/// `CUDA_VISIBLE_DEVICES`. If users have this variable set in their environment this function will
/// likely not return the correct value.
///
/// Therefore you should use the function [`detect_cuda_version_via_nvml`] instead which does not
/// have this limitation.
pub fn detect_cuda_version_via_libcuda() -> Option<Version> {
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
        "libcuda.1.dylib", // Check library path first
        "libcuda.dylib",
        "/usr/local/cuda/lib/libcuda.1.dylib",
        "/usr/local/cuda/lib/libcuda.dylib",
    ];
    #[cfg(target_os = "linux")]
    static FILENAMES: &[&str] = &[
        "libcuda.so.1", // Check library path first
        "libcuda.so",
        "/usr/lib64/nvidia/libcuda.so.1", // RHEL/Centos/Fedora
        "/usr/lib64/nvidia/libcuda.so",
        "/usr/lib/x86_64-linux-gnu/libcuda.so.1", // Ubuntu
        "/usr/lib/x86_64-linux-gnu/libcuda.so",
        "/usr/lib/wsl/lib/libcuda.so.1", // WSL
        "/usr/lib/wsl/lib/libcuda.so",
    ];
    #[cfg(windows)]
    static FILENAMES: &[&str] = &["nvcuda.dll"];
    #[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
    compile_error!("unsupported target os");
    FILENAMES
}

#[cfg(test)]
mod test {
    use super::detect_cuda_version_via_nvml;

    #[test]
    pub fn doesnt_crash() {
        let version = detect_cuda_version_via_nvml();
        println!("Cuda {:?}", version);
    }
}
