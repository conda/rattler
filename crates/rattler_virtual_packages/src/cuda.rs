//! Provides functionality to detect CUDA information present on the current system.
//!
//! This module detects two types of CUDA information:
//!
//! ## CUDA Driver Version (`__cuda`)
//!
//! The CUDA driver version represents the maximum CUDA version supported by the installed
//! NVIDIA drivers. This is detected via:
//!
//! * CUDA driver library (libcuda) - Standard method
//! * nvidia-smi command - Fallback on musl systems where dynamic library loading is not supported
//!
//! ## CUDA Compute Capability (`__cuda_arch`)
//!
//! The CUDA compute capability (also known as SM version or architecture version) represents
//! the **minimum** compute capability of all CUDA devices detected on the system.

use libloading::{Library, Symbol};
use once_cell::sync::OnceCell;
use rattler_conda_types::Version;
use std::process::Command;
use std::{
    ffi::CStr,
    mem::MaybeUninit,
    os::raw::{c_int, c_uint, c_ulong},
    str::FromStr,
};

/// Maximum length for device names in build strings to comply with CEP-26.
const MAX_BUILD_STRING_LEN: usize = 64;

/// Checks if a character is valid in a conda build string according to CEP-26.
///
/// Valid characters are: alphanumeric only (a-z, A-Z, 0-9).
fn is_valid_build_string_char(c: char) -> bool {
    c.is_ascii_alphanumeric()
}

/// Sanitizes a device name to comply with CEP-26 build string requirements.
///
/// This function:
/// 1. Filters out any characters not allowed in build strings (keeps only alphanumeric)
/// 2. Removes "NVIDIA" anywhere in the string (case-insensitive) to save space
/// 3. Truncates to maximum 64 characters
///
/// Returns the sanitized device name.
pub(crate) fn sanitize_device_name(name: &str) -> String {
    // First, filter to keep only alphanumeric characters
    let alphanumeric_only: String = name
        .chars()
        .filter(|c| is_valid_build_string_char(*c))
        .collect();

    // Remove "NVIDIA" anywhere in the string (case-insensitive)
    let without_nvidia = remove_nvidia_ci(&alphanumeric_only);

    // Truncate to maximum length
    without_nvidia.chars().take(MAX_BUILD_STRING_LEN).collect()
}

/// Removes all case-insensitive occurrences of "NVIDIA" from a string.
fn remove_nvidia_ci(s: &str) -> String {
    let mut result = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();

    while let Some(ch) = chars.next() {
        // Check if we're at the start of "NVIDIA" (case-insensitive)
        if ch.eq_ignore_ascii_case(&'N') {
            // Peek ahead to see if "VIDIA" follows
            let upcoming: Vec<char> = chars.clone().take(5).collect();
            if upcoming.len() == 5
                && upcoming[0].eq_ignore_ascii_case(&'V')
                && upcoming[1].eq_ignore_ascii_case(&'I')
                && upcoming[2].eq_ignore_ascii_case(&'D')
                && upcoming[3].eq_ignore_ascii_case(&'I')
                && upcoming[4].eq_ignore_ascii_case(&'A')
            {
                // Skip the next 5 characters ("VIDIA")
                for _ in 0..5 {
                    chars.next();
                }
                continue;
            }
        }
        result.push(ch);
    }

    result
}

/// Validates that a string is in the format "major.minor" where both parts are digits.
///
/// Returns `true` if the format is valid for CUDA compute capability.
pub(crate) fn is_valid_cuda_version_format(s: &str) -> bool {
    let mut parts = s.split('.');
    match (parts.next(), parts.next(), parts.next()) {
        (Some(major), Some(minor), None) => {
            !major.is_empty()
                && major.chars().all(|c| c.is_ascii_digit())
                && !minor.is_empty()
                && minor.chars().all(|c| c.is_ascii_digit())
        }
        _ => false,
    }
}

/// Information about CUDA compute capability for a specific device.
///
/// The compute capability (also called SM version) defines the set of features and
/// instructions supported by a CUDA device. Higher compute capabilities generally
/// support more features and newer instruction sets.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CudaArchInfo {
    /// Major version of the compute capability (e.g., 8 for compute capability 8.6)
    pub major: u32,
    /// Minor version of the compute capability (e.g., 6 for compute capability 8.6)
    pub minor: u32,
    /// Human-readable name of the device (e.g., "NVIDIA `GeForce` RTX 3090")
    pub device_name: String,
}

/// Combined CUDA information detected from the system.
///
/// This struct contains both the CUDA driver version and compute capability information.
/// Each field is optional because detection can fail independently:
///
/// * `version` may be present even without GPUs (driver installed but no devices)
/// * `arch_info` requires at least one GPU device to be present
/// * Both may be `None` if CUDA is not available or detection fails
#[derive(Debug, Clone)]
pub struct CudaInfo {
    /// The maximum CUDA version supported by the installed driver.
    ///
    /// This corresponds to the `__cuda` virtual package.
    pub version: Option<Version>,

    /// Information about the minimum compute capability across all detected devices.
    ///
    /// This corresponds to the `__cuda_arch` virtual package. Returns `None` if
    /// no CUDA devices are detected or if device enumeration fails.
    pub arch_info: Option<CudaArchInfo>,
}

/// Returns comprehensive CUDA information from the current platform.
///
/// This function returns both the CUDA driver version and compute capability information
/// in a single cached result. The detection is performed only once per process and the
/// result is cached for subsequent calls.
///
/// This is more efficient than calling [`cuda_version`] and [`cuda_arch`] separately
/// because the CUDA library is loaded only once.
pub fn cuda_info() -> &'static CudaInfo {
    static DETECTED_CUDA_INFO: OnceCell<CudaInfo> = OnceCell::new();
    DETECTED_CUDA_INFO.get_or_init(detect_cuda_info)
}

/// Returns the maximum CUDA version available on the current platform.
///
/// This corresponds to the `__cuda` virtual package. The result is cached,
/// so subsequent calls are very fast.
pub fn cuda_version() -> Option<Version> {
    cuda_info().version.clone()
}

/// Returns CUDA compute capability information from the current platform.
///
/// This function returns the **minimum** compute capability across all detected
/// CUDA devices, along with the name of the device that has this minimum capability.
///
/// Returns `None` if:
/// * No CUDA drivers are installed
/// * No CUDA devices are detected
/// * Device enumeration fails
/// * The system is using musl libc (dynamic library loading not supported)
///
/// The result is cached, so subsequent calls are very fast.
pub fn cuda_arch() -> Option<CudaArchInfo> {
    cuda_info().arch_info.clone()
}

/// Detects comprehensive CUDA information from the current system.
///
/// This function performs unified detection of both CUDA driver version and compute
/// capability by loading the CUDA library once and querying all necessary information.
///
/// The detection process:
/// 1. Attempts to load the CUDA driver library (`libcuda`)
/// 2. Initializes the CUDA driver API
/// 3. Queries the driver version (for `__cuda` virtual package)
/// 4. Enumerates all CUDA devices and queries their compute capabilities
/// 5. Returns the minimum compute capability across all devices (for `__cuda_arch` virtual package)
///
/// On musl systems, only the version is detected via `nvidia-smi` since dynamic library
/// loading is not supported.
fn detect_cuda_info() -> CudaInfo {
    if cfg!(target_env = "musl") {
        // Dynamically loading a library is not supported on musl so we have to fall-back to using
        // the nvidia-smi command. Architecture detection requires library loading, so it's
        // unavailable on musl.
        CudaInfo {
            version: detect_cuda_version_via_nvidia_smi(),
            arch_info: None,
        }
    } else {
        // Try to detect via libcuda which allows us to get both version and architecture info
        detect_cuda_info_via_libcuda()
    }
}

/// Detects CUDA version and architecture information via the CUDA driver library.
///
/// This function loads `libcuda` and uses the CUDA Driver API to query both the driver
/// version and device compute capabilities. This is more efficient than separate detection
/// because the library is loaded only once.
///
/// Returns a `CudaInfo` struct where:
/// * `version` is `None` if the driver version cannot be determined
/// * `arch_info` is `None` if no devices are present or device queries fail
fn detect_cuda_info_via_libcuda() -> CudaInfo {
    // Try to open the CUDA library
    let cuda_library = match cuda_library_paths()
        .iter()
        .find_map(|path| unsafe { Library::new(*path).ok() })
    {
        Some(lib) => lib,
        None => {
            return CudaInfo {
                version: None,
                arch_info: None,
            }
        }
    };

    // Get entry points from the library
    let cu_init: Symbol<'_, unsafe extern "C" fn(c_uint) -> c_ulong> =
        match unsafe { cuda_library.get(b"cuInit\0") } {
            Ok(init) => init,
            Err(_) => {
                return CudaInfo {
                    version: None,
                    arch_info: None,
                }
            }
        };

    // Initialize the CUDA library
    if unsafe { cu_init(0) } != 0 {
        return CudaInfo {
            version: None,
            arch_info: None,
        };
    }

    // Detect the driver version (can succeed even without devices)
    let version = detect_cuda_version_from_library(&cuda_library);

    // Detect architecture info (requires devices to be present)
    let arch_info = detect_cuda_arch_from_library(&cuda_library);

    CudaInfo { version, arch_info }
}

/// Detects CUDA driver version from an already-loaded CUDA library.
///
/// This function queries the CUDA driver version using `cuDriverGetVersion`.
/// The version can be detected even if no GPU devices are present on the system.
fn detect_cuda_version_from_library(cuda_library: &Library) -> Option<Version> {
    let cu_driver_get_version: Symbol<'_, unsafe extern "C" fn(*mut c_int) -> c_ulong> =
        unsafe { cuda_library.get(b"cuDriverGetVersion\0") }.ok()?;

    // Get the version from the library
    let mut version_int = MaybeUninit::uninit();
    if unsafe { cu_driver_get_version(version_int.as_mut_ptr()) != 0 } {
        return None;
    }
    let version = unsafe { version_int.assume_init() };

    // Convert the version integer to a version string
    Version::from_str(&format!("{}.{}", version / 1000, (version % 1000) / 10)).ok()
}

/// Detects CUDA compute capability from an already-loaded CUDA library.
///
/// This function enumerates all CUDA devices and queries their compute capabilities,
/// returning the **minimum** compute capability found across all devices along with
/// the name of the device that has this minimum capability.
///
/// Returns `None` if:
/// * No CUDA devices are detected (`cuDeviceGetCount` returns 0)
/// * Device enumeration fails
/// * Any of the required CUDA Driver API functions cannot be loaded
fn detect_cuda_arch_from_library(cuda_library: &Library) -> Option<CudaArchInfo> {
    // CUDA device attribute constants for querying compute capability
    const CU_DEVICE_ATTRIBUTE_COMPUTE_CAPABILITY_MAJOR: c_int = 75;
    const CU_DEVICE_ATTRIBUTE_COMPUTE_CAPABILITY_MINOR: c_int = 76;

    // Maximum device name length in CUDA
    const MAX_DEVICE_NAME_LEN: usize = 256;

    // Get required function pointers from the library
    let cu_device_get_count: Symbol<'_, unsafe extern "C" fn(*mut c_int) -> c_ulong> =
        unsafe { cuda_library.get(b"cuDeviceGetCount\0") }.ok()?;

    let cu_device_get: Symbol<'_, unsafe extern "C" fn(*mut c_int, c_int) -> c_ulong> =
        unsafe { cuda_library.get(b"cuDeviceGet\0") }.ok()?;

    let cu_device_get_attribute: Symbol<
        '_,
        unsafe extern "C" fn(*mut c_int, c_int, c_int) -> c_ulong,
    > = unsafe { cuda_library.get(b"cuDeviceGetAttribute\0") }.ok()?;

    let cu_device_get_name: Symbol<'_, unsafe extern "C" fn(*mut u8, c_int, c_int) -> c_ulong> =
        unsafe { cuda_library.get(b"cuDeviceGetName\0") }.ok()?;

    // Get the number of CUDA devices
    let mut device_count = MaybeUninit::uninit();
    if unsafe { cu_device_get_count(device_count.as_mut_ptr()) } != 0 {
        return None;
    }
    let device_count = unsafe { device_count.assume_init() };

    // No devices found
    if device_count == 0 {
        return None;
    }

    // Iterate through all devices to find the minimum compute capability
    let mut min_arch: Option<CudaArchInfo> = None;

    for device_idx in 0..device_count {
        // Get device handle
        let mut device = MaybeUninit::uninit();
        if unsafe { cu_device_get(device.as_mut_ptr(), device_idx) } != 0 {
            continue;
        }
        let device = unsafe { device.assume_init() };

        // Get compute capability major version
        let mut cc_major = MaybeUninit::uninit();
        if unsafe {
            cu_device_get_attribute(
                cc_major.as_mut_ptr(),
                CU_DEVICE_ATTRIBUTE_COMPUTE_CAPABILITY_MAJOR,
                device,
            )
        } != 0
        {
            continue;
        }
        let cc_major = unsafe { cc_major.assume_init() } as u32;

        // Get compute capability minor version
        let mut cc_minor = MaybeUninit::uninit();
        if unsafe {
            cu_device_get_attribute(
                cc_minor.as_mut_ptr(),
                CU_DEVICE_ATTRIBUTE_COMPUTE_CAPABILITY_MINOR,
                device,
            )
        } != 0
        {
            continue;
        }
        let cc_minor = unsafe { cc_minor.assume_init() } as u32;

        // Check if this is the minimum compute capability so far
        let is_new_minimum = min_arch.as_ref().is_none_or(|min| {
            cc_major < min.major || (cc_major == min.major && cc_minor < min.minor)
        });

        if is_new_minimum {
            // Get device name
            let mut name_buffer = [0u8; MAX_DEVICE_NAME_LEN];
            if unsafe {
                cu_device_get_name(
                    name_buffer.as_mut_ptr(),
                    MAX_DEVICE_NAME_LEN as c_int,
                    device,
                )
            } == 0
            {
                // Convert C string to Rust string and sanitize
                if let Ok(cstr) = CStr::from_bytes_until_nul(&name_buffer) {
                    if let Ok(device_name) = cstr.to_str() {
                        min_arch = Some(CudaArchInfo {
                            major: cc_major,
                            minor: cc_minor,
                            device_name: sanitize_device_name(device_name),
                        });
                    }
                }
            }
        }
    }

    min_arch
}

/// Attempts to detect the version of CUDA present in the current operating system by employing the
/// best technique available for the current environment.
pub fn detect_cuda_version() -> Option<Version> {
    if cfg!(target_env = "musl") {
        // Dynamically loading a library is not supported on musl so we have to fall-back to using
        // the nvidia-smi command.
        detect_cuda_version_via_nvidia_smi()
    } else {
        detect_cuda_version_via_nvml()
    }
}

/// Attempts to detect the version of CUDA present in the current operating system by loading the
/// NVIDIA Management Library and querying the CUDA driver version. The method is preferred over
/// [`detect_cuda_version_via_libcuda`] because that method might fail base on environment
/// variables.
///
/// Although the required methods in the runtime are not implemented on much older machines it is
/// considered old enough to be usable for our use case. Since Conda doesn't provide old versions of
/// the CUDA SDK anyway this is considered a non-issue.
pub fn detect_cuda_version_via_nvml() -> Option<Version> {
    // Try to open the library
    let library = nvml_library_paths()
        .iter()
        .find_map(|path| unsafe { libloading::Library::new(*path).ok() })?;

    // Get the initialization function. We first try to get `nvmlInit_v2` but if we can't find that
    // we use the `nvmlInit` function.
    let nvml_init: Symbol<'_, unsafe extern "C" fn() -> c_int> = unsafe {
        library
            .get(b"nvmlInit_v2\0")
            .or_else(|_| library.get(b"nvmlInit\0"))
    }
    .ok()?;

    // Find the shutdown function
    let nvml_shutdown: Symbol<'_, unsafe extern "C" fn() -> c_int> =
        unsafe { library.get(b"nvmlShutdown\0") }.ok()?;

    // Find the `nvmlSystemGetCudaDriverVersion_v2` function. If that function cannot be found, fall
    // back to the `nvmlSystemGetCudaDriverVersion` function instead.
    let nvml_system_get_cuda_driver_version: Symbol<'_, unsafe extern "C" fn(*mut c_int) -> c_int> =
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
    ];
    #[cfg(windows)]
    static FILENAMES: &[&str] = &["nvml.dll"];
    #[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
    static FILENAMES: &[&str] = &[];
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
    let cu_init: Symbol<'_, unsafe extern "C" fn(c_uint) -> c_ulong> =
        unsafe { cuda_library.get(b"cuInit\0") }.ok()?;
    let cu_driver_get_version: Symbol<'_, unsafe extern "C" fn(*mut c_int) -> c_ulong> =
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
    static FILENAMES: &[&str] = &[];
    FILENAMES
}

/// Attempts to detect the version of CUDA present in the current operating system by executing the
/// "nvidia-smi" command and extracting the CUDA driver version from it.
///
/// The behavior of "nvidia-smi" depends on the environment variable `CUDA_VISIBLE_DEVICES`. If
/// users have this variable set in their environment this function will likely not return the
/// correct value. To ensure a consistent response this environment variable is unset when invoking
/// the command.
///
/// The upside of using this detection function over any of the others is that this method does not
/// dynamically load a library which might not be supported on all systems. The downside is that
/// executing a subprocess is generally slower and more prone to errors.
fn detect_cuda_version_via_nvidia_smi() -> Option<Version> {
    static CUDA_VERSION_RE: once_cell::sync::Lazy<regex::Regex> =
        once_cell::sync::Lazy::new(|| {
            regex::Regex::new("<cuda_version>(.*)<\\/cuda_version>").unwrap()
        });

    // Invoke the "nvidia-smi" command to query the driver version that is usually installed when
    // Cuda drivers are installed.
    let nvidia_smi_output = Command::new("nvidia-smi")
        // Display GPU or unit info
        .arg("--query")
        // Show unit, rather than GPU, attributes
        .arg("-u")
        // Produce XML output.
        .arg("-x")
        // The behavior of functions from `libcuda` depend on the environment variable
        // `CUDA_VISIBLE_DEVICES`. If users have this variable set in their environment this
        // function will likely not return the correct value. Therefor, we remove this variable
        // to ensure a consistent result.
        // TODO: Is this really the proper way to do it? Should we maybe clear the entire
        // environment.
        .env_remove("CUDA_VISIBLE_DEVICES")
        .output()
        .ok()?;

    // Convert the output to Utf8. The conversion is lossy so it might contain some illegal
    // characters. If that is the case we simply assume the version in the file also wont make sense
    // during parsing.
    let output = String::from_utf8_lossy(&nvidia_smi_output.stdout);

    // Extract the version from the XML
    let version_match = CUDA_VERSION_RE.captures(&output)?;
    let version_str = version_match.get(1)?.as_str();

    // Parse and return
    Version::from_str(version_str).ok()
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    pub fn doesnt_crash() {
        let version = detect_cuda_version_via_nvml();
        println!("Cuda {version:?}");
    }

    #[test]
    pub fn doesnt_crash_nvidia_smi() {
        let version = detect_cuda_version_via_nvidia_smi();
        println!("Cuda {version:?}");
    }

    #[test]
    pub fn test_cuda_info() {
        let info = cuda_info();
        println!("CUDA Info: {info:?}");
        if let Some(ref arch) = info.arch_info {
            println!(
                "  Device: {} (compute {}.{})",
                arch.device_name, arch.major, arch.minor
            );
        }
    }

    #[test]
    pub fn test_cuda_arch() {
        let arch = cuda_arch();
        println!("CUDA Arch: {arch:?}");
    }

    #[test]
    fn test_is_valid_cuda_version_format() {
        // Valid formats
        assert!(is_valid_cuda_version_format("8.6"));
        assert!(is_valid_cuda_version_format("7.5"));
        assert!(is_valid_cuda_version_format("10.2"));
        assert!(is_valid_cuda_version_format("0.0"));
        assert!(is_valid_cuda_version_format("12.0"));

        // Invalid formats - not major.minor
        assert!(!is_valid_cuda_version_format("8"));
        assert!(!is_valid_cuda_version_format("8.6.1"));
        assert!(!is_valid_cuda_version_format("8.6.1.0"));
        assert!(!is_valid_cuda_version_format(""));
        assert!(!is_valid_cuda_version_format(".6"));
        assert!(!is_valid_cuda_version_format("8."));
        assert!(!is_valid_cuda_version_format("."));

        // Invalid formats - non-digit characters
        assert!(!is_valid_cuda_version_format("8.6a"));
        assert!(!is_valid_cuda_version_format("a.6"));
        assert!(!is_valid_cuda_version_format("8.b"));
        assert!(!is_valid_cuda_version_format("eight.six"));
        assert!(!is_valid_cuda_version_format("8-6"));
        assert!(!is_valid_cuda_version_format("8_6"));
    }

    #[test]
    fn test_is_valid_build_string_char() {
        // Valid characters - alphanumeric only
        assert!(is_valid_build_string_char('a'));
        assert!(is_valid_build_string_char('Z'));
        assert!(is_valid_build_string_char('0'));
        assert!(is_valid_build_string_char('9'));

        // Invalid characters - everything else
        assert!(!is_valid_build_string_char(' '));
        assert!(!is_valid_build_string_char('-'));
        assert!(!is_valid_build_string_char('/'));
        assert!(!is_valid_build_string_char('!'));
        assert!(!is_valid_build_string_char('@'));
        assert!(!is_valid_build_string_char('#'));
        assert!(!is_valid_build_string_char('_'));
        assert!(!is_valid_build_string_char('.'));
        assert!(!is_valid_build_string_char('+'));
    }

    #[test]
    fn test_remove_nvidia_ci() {
        // Remove NVIDIA at the start
        assert_eq!(remove_nvidia_ci("NVIDIAGeForce"), "GeForce");
        assert_eq!(remove_nvidia_ci("nvidiaRTX"), "RTX");

        // Remove NVIDIA in the middle
        assert_eq!(remove_nvidia_ci("GeForceNVIDIARTX"), "GeForceRTX");
        assert_eq!(remove_nvidia_ci("TestNVIDIAGPU"), "TestGPU");

        // Remove NVIDIA at the end
        assert_eq!(remove_nvidia_ci("TeslaNVIDIA"), "Tesla");
        assert_eq!(remove_nvidia_ci("GPUnvidia"), "GPU");

        // Multiple occurrences
        assert_eq!(remove_nvidia_ci("NVIDIATestNVIDIA"), "Test");
        assert_eq!(remove_nvidia_ci("nvidianvidianvidia"), "");

        // Case variations
        assert_eq!(remove_nvidia_ci("NvIdIaTest"), "Test");
        assert_eq!(remove_nvidia_ci("TestNVidia"), "Test");

        // No NVIDIA
        assert_eq!(remove_nvidia_ci("AMDRadeon"), "AMDRadeon");
        assert_eq!(remove_nvidia_ci("Intel"), "Intel");

        // Edge cases
        assert_eq!(remove_nvidia_ci(""), "");
        assert_eq!(remove_nvidia_ci("NVIDIA"), "");
        assert_eq!(remove_nvidia_ci("nvidia"), "");

        // Partial matches should not be removed
        assert_eq!(remove_nvidia_ci("NVID"), "NVID");
        assert_eq!(remove_nvidia_ci("NVIDI"), "NVIDI");
        assert_eq!(remove_nvidia_ci("VIDIA"), "VIDIA");
    }

    #[test]
    fn test_sanitize_device_name() {
        // Valid name with NVIDIA prefix - alphanumeric only, NVIDIA removed
        assert_eq!(
            sanitize_device_name("NVIDIA GeForce RTX 3090"),
            "GeForceRTX3090"
        );

        // Different NVIDIA case variations
        assert_eq!(sanitize_device_name("nvidia GeForce"), "GeForce");
        assert_eq!(sanitize_device_name("Nvidia Tesla"), "Tesla");
        assert_eq!(sanitize_device_name("NVIDIA RTX"), "RTX");

        // NVIDIA appearing in the middle or end (not just prefix)
        assert_eq!(sanitize_device_name("GeForce NVIDIA RTX"), "GeForceRTX");
        assert_eq!(sanitize_device_name("Tesla NVIDIA"), "Tesla");
        assert_eq!(sanitize_device_name("nVidia Test nvidia GPU"), "TestGPU");

        // No NVIDIA prefix
        assert_eq!(
            sanitize_device_name("AMD Radeon RX 6800"),
            "AMDRadeonRX6800"
        );
        assert_eq!(sanitize_device_name("Tesla V100"), "TeslaV100");

        // Special characters and spaces - only alphanumeric allowed
        assert_eq!(
            sanitize_device_name("Device-Name_Test.1+2"),
            "DeviceNameTest12"
        );
        assert_eq!(sanitize_device_name("Test @ Device #1!"), "TestDevice1");

        // Length truncation (64 char limit)
        let long_name = "A".repeat(100);
        assert_eq!(sanitize_device_name(&long_name).len(), 64);

        let long_with_nvidia = format!("NVIDIA {}", "A".repeat(100));
        assert_eq!(sanitize_device_name(&long_with_nvidia).len(), 64);

        // Edge cases
        assert_eq!(sanitize_device_name(""), "");
        assert_eq!(sanitize_device_name("NVIDIA"), "");
        assert_eq!(sanitize_device_name("nvidia "), "");
        assert_eq!(sanitize_device_name("   "), "");
        assert_eq!(sanitize_device_name("!@#$%"), "");

        // Only alphanumeric chars allowed now
        assert_eq!(sanitize_device_name("ABC_123.test+v2"), "ABC123testv2");
    }
}
