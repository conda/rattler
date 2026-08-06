//! Provides functionality to detect CUDA information present on the current system.
//!
//! This module detects two types of CUDA information:
//!
//! ## CUDA Driver Version (`__cuda`)
//!
//! The CUDA driver version represents the maximum CUDA version supported by the installed
//! NVIDIA drivers. This is detected via:
//!
//! * NVIDIA Management Library (NVML): Standard method
//! * CUDA driver library (libcuda) and the nvidia-smi command: Fallbacks for systems without
//!   NVML, and for musl systems where dynamic library loading is not supported
//!
//! ## CUDA Compute Capability (`__cuda_arch`)
//!
//! The CUDA compute capability (also known as SM version or architecture version) represents
//! the **minimum** compute capability of all CUDA devices detected on the system.

use libloading::{Library, Symbol};
use once_cell::sync::OnceCell;
use rattler_conda_types::Version;
use serde::{Deserialize, Serialize};
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::{
    os::raw::{c_int, c_uint, c_void},
    path::Path,
    ptr,
    str::FromStr,
};

mod cache;

const NVML_SUCCESS: c_int = 0;
const NVML_ERROR_UNINITIALIZED: c_int = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum NvmlCudaVersionError {
    MissingSymbol,
    Nvml(c_int),
    InvalidVersion,
}

impl NvmlCudaVersionError {
    fn should_retry_after_init(self) -> bool {
        matches!(self, Self::Nvml(NVML_ERROR_UNINITIALIZED))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum CudaDetectionMethod {
    NvmlNoInit,
    NvmlInitialized,
    Libcuda,
    NvidiaSmi,
}

impl CudaDetectionMethod {
    fn as_str(self) -> &'static str {
        match self {
            Self::NvmlNoInit => "nvml_no_init",
            Self::NvmlInitialized => "nvml_initialized",
            Self::Libcuda => "libcuda",
            Self::NvidiaSmi => "nvidia_smi",
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(super) struct CudaInfoSources {
    pub version: Option<CudaDetectionMethod>,
    pub arch: Option<CudaDetectionMethod>,
}

impl CudaInfoSources {
    fn version_str(self) -> &'static str {
        self.version
            .map_or("<unknown>", CudaDetectionMethod::as_str)
    }

    fn arch_str(self) -> &'static str {
        self.arch.map_or("<unknown>", CudaDetectionMethod::as_str)
    }
}

struct DetectedCudaInfo {
    info: CudaInfo,
    sources: CudaInfoSources,
}

/// Converts a CUDA driver version integer (as reported by NVML/libcuda) into a [`Version`].
///
/// The integer is encoded as `major * 1000 + minor * 10` (e.g. `12040` for CUDA 12.4). Because the
/// FFI out-parameters are zero-initialized, an implausible value (such as `0` from an out-param the
/// driver never wrote, a negative value, or a nonsensically large one) can slip through even on a
/// `SUCCESS` return. Only values whose CUDA major version lies in `1..=99` are accepted; anything
/// else is rejected so that garbage never propagates (or gets cached).
fn parse_cuda_driver_version(version: c_int) -> Option<Version> {
    // CUDA major version 1..=99, i.e. the encoded integer must be within [1000, 99990].
    if !(1000..=99_990).contains(&version) {
        tracing::trace!(version, "rejecting implausible CUDA driver version integer");
        return None;
    }
    Version::from_str(&format!("{}.{}", version / 1000, (version % 1000) / 10)).ok()
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

/// Information about CUDA compute capability.
///
/// The compute capability (also called SM version) defines the set of features and
/// instructions supported by a CUDA device. Higher compute capabilities generally
/// support more features and newer instruction sets.
///
/// According to the CEP specification, this represents the minimum compute capability
/// across all detected CUDA devices, formatted as `{major}.{minor}`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CudaArchInfo {
    /// Major version of the compute capability (e.g., 8 for compute capability 8.6)
    pub major: u32,
    /// Minor version of the compute capability (e.g., 6 for compute capability 8.6)
    pub minor: u32,
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

fn display_cuda_version(version: Option<&Version>) -> String {
    version.map_or_else(|| "<none>".to_string(), ToString::to_string)
}

fn display_cuda_arch(arch_info: Option<&CudaArchInfo>) -> String {
    arch_info.map_or_else(
        || "<none>".to_string(),
        |arch| format!("{}.{}", arch.major, arch.minor),
    )
}

/// Returns comprehensive CUDA information from the current platform.
///
/// This function returns both the CUDA driver version and compute capability information
/// in a single cached result. The detection is performed only once per process and the
/// result is cached for subsequent calls.
///
/// This is more efficient than calling [`cuda_version`] and [`cuda_arch`] separately
/// because the CUDA library is loaded only once.
///
/// Detection runs at most once per process; the in-memory result is reused afterwards. The on-disk
/// cache, however, is synced lazily: the first call that is given a `cache_dir` reads from and/or
/// writes to it. A call that passes `None` (e.g. `EnvOverride::detect_from_host`) does not disable
/// the disk cache for the rest of the process — a later call passing `Some(cache_dir)` still
/// persists the already-detected result. Pass `None` from every call to fully disable the disk
/// cache.
pub fn cuda_info(cache_dir: Option<&Path>) -> &'static CudaInfo {
    static DETECTED_CUDA_INFO: OnceCell<DetectedCudaInfo> = OnceCell::new();
    // Whether the in-memory result has been synced with the on-disk cache (read from it or written
    // to it). This lets a later call with a `cache_dir` persist a result first detected without one.
    static PERSISTED: AtomicBool = AtomicBool::new(false);
    cuda_info_impl(
        &cache::CacheEnv::current(),
        &DETECTED_CUDA_INFO,
        &PERSISTED,
        cache_dir,
    )
}

/// Core of [`cuda_info`], generic over the state so it can be unit-tested with local state instead
/// of the process-global statics.
fn cuda_info_impl<'a>(
    env: &cache::CacheEnv,
    state: &'a OnceCell<DetectedCudaInfo>,
    persisted: &AtomicBool,
    cache_dir: Option<&Path>,
) -> &'a CudaInfo {
    if let Some(detected) = state.get() {
        tracing::trace!(info = ?detected.info, "using process-cached CUDA info");
        maybe_persist(env, detected, persisted, cache_dir);
        return &detected.info;
    }

    let detected = state.get_or_init(|| {
        // Initializing the driver to detect the GPU can be slow, so the result is cached on disk
        // and reused until the cache is invalidated (reboot, driver change, GPU change, TTL).
        if let Some(cache_dir) = cache_dir {
            tracing::trace!(cache_dir = %cache_dir.display(), "checking CUDA info cache");
            if let Some(cached) = cache::read_with_env(env, cache_dir) {
                tracing::debug!(
                    version = %display_cuda_version(cached.info.version.as_ref()),
                    arch = %display_cuda_arch(cached.info.arch_info.as_ref()),
                    version_source = cached.sources.version_str(),
                    arch_source = cached.sources.arch_str(),
                    "using disk-cached CUDA info"
                );
                // We are now in sync with disk; no need to write it back.
                persisted.store(true, Ordering::Relaxed);
                return cached;
            }
        } else {
            tracing::trace!("CUDA info disk cache disabled");
        }

        tracing::trace!("detecting CUDA info from host");
        let detected = detect_cuda_info();
        tracing::debug!(
            version = %display_cuda_version(detected.info.version.as_ref()),
            arch = %display_cuda_arch(detected.info.arch_info.as_ref()),
            version_source = detected.sources.version_str(),
            arch_source = detected.sources.arch_str(),
            "detected CUDA info from host"
        );
        detected
    });

    // Persist a freshly detected result if this (or a later) call supplied a cache directory.
    maybe_persist(env, detected, persisted, cache_dir);
    &detected.info
}

/// Writes the detected result to disk once, if a cache directory is available and it has not been
/// synced with disk yet. A benign race may write twice, which is safe because the write replaces
/// the file atomically.
fn maybe_persist(
    env: &cache::CacheEnv,
    detected: &DetectedCudaInfo,
    persisted: &AtomicBool,
    cache_dir: Option<&Path>,
) {
    let Some(cache_dir) = cache_dir else {
        return;
    };
    if persisted.load(Ordering::Relaxed) {
        return;
    }
    cache::write_with_env(env, cache_dir, &detected.info, detected.sources);
    // The flag means "we have synced with disk"; set it regardless of write's best-effort outcome.
    persisted.store(true, Ordering::Relaxed);
}

/// Returns the maximum CUDA version available on the current platform.
///
/// This corresponds to the `__cuda` virtual package. The result is cached,
/// so subsequent calls are very fast. See [`cuda_info`] for the `cache_dir` semantics.
pub fn cuda_version(cache_dir: Option<&Path>) -> Option<Version> {
    cuda_info(cache_dir).version.clone()
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
///
/// The result is cached, so subsequent calls are very fast. See [`cuda_info`] for the `cache_dir`
/// semantics.
pub fn cuda_arch(cache_dir: Option<&Path>) -> Option<CudaArchInfo> {
    cuda_info(cache_dir).arch_info.clone()
}

/// Detects comprehensive CUDA information from the current system.
///
/// This function performs unified detection of both CUDA driver version and compute
/// capability by loading NVML once and querying all necessary information.
///
/// The detection process:
/// 1. Attempts to load NVML (`libnvidia-ml`/`nvml.dll`)
/// 2. Queries the driver version (for `__cuda` virtual package); this does not require init
/// 3. Initializes NVML and enumerates all CUDA devices to query their compute capabilities
/// 4. Returns the minimum compute capability across all devices (for `__cuda_arch` virtual package)
///
/// On musl systems, both are detected via the `nvidia-smi` command since dynamic library
/// loading is not supported.
fn detect_cuda_info() -> DetectedCudaInfo {
    let mut detected = if cfg!(target_env = "musl") {
        tracing::trace!("detecting CUDA info via nvidia-smi because musl cannot load NVML");
        // Dynamically loading a library is not supported on musl so we have to fall-back to using
        // the nvidia-smi command.
        let version = detect_cuda_version_via_nvidia_smi();
        // Only query the compute capability when the driver version was found. On a GPU-less musl
        // system the version query already failed, so running the arch query too would just spawn
        // another doomed process and could produce an inconsistent `{version: None, arch: Some}`.
        let arch_info = version
            .as_ref()
            .and_then(|_| detect_cuda_arch_via_nvidia_smi());
        DetectedCudaInfo {
            sources: CudaInfoSources {
                version: version.is_some().then_some(CudaDetectionMethod::NvidiaSmi),
                arch: arch_info
                    .is_some()
                    .then_some(CudaDetectionMethod::NvidiaSmi),
            },
            info: CudaInfo { version, arch_info },
        }
    } else {
        tracing::trace!("detecting CUDA info via NVML");
        // Prefer NVML because it is not affected by `CUDA_VISIBLE_DEVICES`, but fall back to the
        // older probes so systems that expose libcuda (or nvidia-smi) without NVML still report
        // `__cuda`.
        let mut detected = detect_cuda_info_via_nvml();

        if detected.info.version.is_none() {
            tracing::debug!(
                "NVML did not detect a CUDA driver version; trying libcuda/nvidia-smi fallbacks"
            );
            if let Some((version, source)) = detect_cuda_version_fallbacks() {
                detected.info.version = Some(version);
                detected.sources.version = Some(source);
            }
        }

        if detected.info.version.is_some() && detected.info.arch_info.is_none() {
            tracing::debug!(
                "NVML did not detect CUDA compute capability; trying nvidia-smi fallback"
            );
            detected.info.arch_info = detect_cuda_arch_via_nvidia_smi();
            if detected.info.arch_info.is_some() {
                detected.sources.arch = Some(CudaDetectionMethod::NvidiaSmi);
            }
        }

        // Last resort for platforms that ship libcuda but neither NVML nor nvidia-smi (e.g.
        // Jetson/Tegra). libcuda comes last because its device enumeration is affected by
        // `CUDA_VISIBLE_DEVICES`.
        if detected.info.version.is_some() && detected.info.arch_info.is_none() {
            tracing::debug!(
                "nvidia-smi did not detect CUDA compute capability; trying libcuda fallback"
            );
            detected.info.arch_info = detect_cuda_arch_via_libcuda();
            if detected.info.arch_info.is_some() {
                detected.sources.arch = Some(CudaDetectionMethod::Libcuda);
            }
        }

        detected
    };

    // Normalization for all paths: `__cuda_arch` is meaningless without `__cuda`. If no driver
    // version was detected, drop any compute capability so callers can never observe
    // arch-without-version.
    if detected.info.version.is_none()
        && (detected.info.arch_info.is_some() || detected.sources.arch.is_some())
    {
        tracing::debug!(
            "dropping CUDA compute capability because no CUDA driver version was detected"
        );
        detected.info.arch_info = None;
        detected.sources.arch = None;
    }

    detected
}

/// Detects CUDA version and architecture information via the NVIDIA Management Library.
///
/// The library is loaded once and used to query both the driver version and device compute
/// capabilities. NVML is preferred over libcuda because it is not affected by
/// `CUDA_VISIBLE_DEVICES`.
///
/// Returns a `CudaInfo` struct where:
/// * `version` is `None` if the driver version cannot be determined
/// * `arch_info` is `None` if no devices are present or device queries fail
fn detect_cuda_info_via_nvml() -> DetectedCudaInfo {
    let mut library = None;
    for path in nvml_library_paths() {
        match unsafe { Library::new(*path) } {
            Ok(loaded) => {
                tracing::trace!(library_path = *path, "loaded NVML library");
                library = Some(loaded);
                break;
            }
            Err(err) => {
                tracing::trace!(library_path = *path, error = %err, "failed to load NVML library");
            }
        }
    }

    let Some(library) = library else {
        tracing::debug!("could not load NVML library from any known path");
        return DetectedCudaInfo {
            info: CudaInfo {
                version: None,
                arch_info: None,
            },
            sources: CudaInfoSources::default(),
        };
    };

    // Attempt the cheap no-init query. Some drivers answer `nvmlSystemGetCudaDriverVersion` before
    // `nvmlInit`, but it is not officially supported, so this result is only used as a fall back for
    // when `nvmlInit` itself fails below.
    let no_init_version = match cuda_version_from_nvml_library(&library) {
        Ok(version) => {
            tracing::trace!(%version, "detected CUDA driver version via NVML without init");
            Some(version)
        }
        Err(err) => {
            tracing::trace!(
                ?err,
                "CUDA driver version query via NVML without init failed"
            );
            None
        }
    };

    // Compute capability requires enumerating devices, which needs NVML to be initialized. Since
    // NVML is being initialized anyway, query the driver version while initialized too and prefer
    // that officially supported result over the no-init query.
    let (initialized_version, arch_info) =
        detect_cuda_initialized_info_via_nvml(&library, true, true);

    // Prefer the version obtained from initialized NVML whenever it is available; only fall back to
    // the no-init value when `nvmlInit` (and thus the initialized query) did not produce one.
    let (version, version_source) = if let Some(version) = initialized_version {
        (Some(version), Some(CudaDetectionMethod::NvmlInitialized))
    } else if let Some(version) = no_init_version {
        (Some(version), Some(CudaDetectionMethod::NvmlNoInit))
    } else {
        (None, None)
    };

    let arch_source = arch_info
        .as_ref()
        .map(|_| CudaDetectionMethod::NvmlInitialized);

    DetectedCudaInfo {
        info: CudaInfo { version, arch_info },
        sources: CudaInfoSources {
            version: version_source,
            arch: arch_source,
        },
    }
}

/// Queries the CUDA driver version from an already-loaded NVML library.
///
/// Some drivers allow `nvmlSystemGetCudaDriverVersion` before `nvmlInit`, but others return
/// `NVML_ERROR_UNINITIALIZED`; callers may retry after initializing NVML for that error.
fn cuda_version_from_nvml_library(library: &Library) -> Result<Version, NvmlCudaVersionError> {
    // Find the `nvmlSystemGetCudaDriverVersion_v2` function. If that function cannot be found, fall
    // back to the `nvmlSystemGetCudaDriverVersion` function instead.
    let nvml_system_get_cuda_driver_version: Symbol<'_, unsafe extern "C" fn(*mut c_int) -> c_int> =
        unsafe {
            library
                .get(b"nvmlSystemGetCudaDriverVersion_v2\0")
                .or_else(|_| library.get(b"nvmlSystemGetCudaDriverVersion\0"))
        }
        .map_err(|_err| NvmlCudaVersionError::MissingSymbol)?;

    // Zero-initialize the out-parameter so that a driver returning `NVML_SUCCESS` without actually
    // writing it yields a deterministic `0`, which `parse_cuda_driver_version` rejects, rather than
    // undefined behavior from reading uninitialized memory.
    let mut cuda_driver_version: c_int = 0;
    let result = unsafe { nvml_system_get_cuda_driver_version(&mut cuda_driver_version) };
    if result != NVML_SUCCESS {
        return Err(NvmlCudaVersionError::Nvml(result));
    }

    parse_cuda_driver_version(cuda_driver_version).ok_or(NvmlCudaVersionError::InvalidVersion)
}

/// Queries information that requires initialized NVML.
///
/// Returns `(version, arch_info)`. Each field is only queried when the corresponding `query_*`
/// argument is true. The initialized version query is a compatibility fallback for drivers that
/// reject `nvmlSystemGetCudaDriverVersion` before `nvmlInit`.
fn detect_cuda_initialized_info_via_nvml(
    library: &Library,
    query_version: bool,
    query_arch: bool,
) -> (Option<Version>, Option<CudaArchInfo>) {
    // NVML device handle (`nvmlDevice_t`) is an opaque pointer.
    type NvmlDevice = *mut c_void;

    let Some(nvml_init): Option<Symbol<'_, unsafe extern "C" fn() -> c_int>> = (unsafe {
        library
            .get(b"nvmlInit_v2\0")
            .or_else(|_| library.get(b"nvmlInit\0"))
            .ok()
    }) else {
        tracing::debug!("missing nvmlInit symbol");
        return (None, None);
    };

    let Some(nvml_shutdown): Option<Symbol<'_, unsafe extern "C" fn() -> c_int>> =
        (unsafe { library.get(b"nvmlShutdown\0").ok() })
    else {
        tracing::debug!("missing nvmlShutdown symbol");
        return (None, None);
    };

    tracing::trace!(query_version, query_arch, "initializing NVML");
    let init_result = unsafe { nvml_init() };
    if init_result != NVML_SUCCESS {
        tracing::debug!(return_code = init_result, "nvmlInit failed");
        return (None, None);
    }

    let version = if query_version {
        match cuda_version_from_nvml_library(library) {
            Ok(version) => {
                tracing::debug!(%version, "detected CUDA driver version via initialized NVML");
                Some(version)
            }
            Err(err) => {
                tracing::debug!(
                    ?err,
                    "CUDA driver version query via initialized NVML failed"
                );
                None
            }
        }
    } else {
        None
    };

    // Enumerate devices to find the minimum compute capability. Wrapped in a closure so we always
    // reach the `nvmlShutdown` call below regardless of the outcome.
    let arch_info = query_arch
        .then(|| {
            tracing::trace!("querying CUDA compute capability via NVML");
            let nvml_device_get_count: Symbol<'_, unsafe extern "C" fn(*mut c_uint) -> c_int> =
                match unsafe {
                    library
                        .get(b"nvmlDeviceGetCount_v2\0")
                        .or_else(|_| library.get(b"nvmlDeviceGetCount\0"))
                } {
                    Ok(symbol) => symbol,
                    Err(err) => {
                        tracing::trace!(error = %err, "missing NVML device count symbol");
                        return None;
                    }
                };

            let nvml_device_get_handle_by_index: Symbol<
                '_,
                unsafe extern "C" fn(c_uint, *mut NvmlDevice) -> c_int,
            > = match unsafe {
                library
                    .get(b"nvmlDeviceGetHandleByIndex_v2\0")
                    .or_else(|_| library.get(b"nvmlDeviceGetHandleByIndex\0"))
            } {
                Ok(symbol) => symbol,
                Err(err) => {
                    tracing::trace!(error = %err, "missing NVML device handle symbol");
                    return None;
                }
            };

            let nvml_device_get_cuda_compute_capability: Symbol<
                '_,
                unsafe extern "C" fn(NvmlDevice, *mut c_int, *mut c_int) -> c_int,
            > = match unsafe { library.get(b"nvmlDeviceGetCudaComputeCapability\0") } {
                Ok(symbol) => symbol,
                Err(err) => {
                    tracing::trace!(
                        error = %err,
                        "missing NVML CUDA compute capability symbol"
                    );
                    return None;
                }
            };

            let mut device_count: c_uint = 0;
            let device_count_result = unsafe { nvml_device_get_count(&mut device_count) };
            if device_count_result != NVML_SUCCESS {
                tracing::trace!(
                    return_code = device_count_result,
                    "nvmlDeviceGetCount failed"
                );
                return None;
            }
            tracing::trace!(device_count, "enumerating CUDA devices via NVML");

            let mut min_arch: Option<CudaArchInfo> = None;
            for device_idx in 0..device_count {
                let mut device: NvmlDevice = ptr::null_mut();
                let handle_result =
                    unsafe { nvml_device_get_handle_by_index(device_idx, &mut device) };
                if handle_result != NVML_SUCCESS {
                    tracing::trace!(
                        device_idx,
                        return_code = handle_result,
                        "failed to get NVML device handle"
                    );
                    continue;
                }

                let mut cc_major: c_int = 0;
                let mut cc_minor: c_int = 0;
                let compute_capability_result = unsafe {
                    nvml_device_get_cuda_compute_capability(device, &mut cc_major, &mut cc_minor)
                };
                if compute_capability_result != NVML_SUCCESS {
                    tracing::trace!(
                        device_idx,
                        return_code = compute_capability_result,
                        "failed to get CUDA compute capability via NVML"
                    );
                    continue;
                }
                let cc_major = cc_major as u32;
                let cc_minor = cc_minor as u32;
                tracing::trace!(
                    device_idx,
                    major = cc_major,
                    minor = cc_minor,
                    "detected CUDA compute capability via NVML"
                );

                let is_new_minimum = min_arch.as_ref().is_none_or(|min| {
                    cc_major < min.major || (cc_major == min.major && cc_minor < min.minor)
                });
                if is_new_minimum {
                    min_arch = Some(CudaArchInfo {
                        major: cc_major,
                        minor: cc_minor,
                    });
                }
            }
            if let Some(arch) = min_arch.as_ref() {
                tracing::debug!(
                    major = arch.major,
                    minor = arch.minor,
                    "selected minimum CUDA compute capability"
                );
            } else {
                tracing::debug!("no CUDA compute capability detected via NVML");
            }
            min_arch
        })
        .flatten();

    // Whatever happens, after initializing NVML we have to call `nvmlShutdown`.
    let shutdown_result = unsafe { nvml_shutdown() };
    if shutdown_result != NVML_SUCCESS {
        tracing::debug!(return_code = shutdown_result, "nvmlShutdown failed");
    }

    (version, arch_info)
}

/// Attempts to detect the version of CUDA present in the current operating system by employing the
/// best technique available for the current environment.
pub fn detect_cuda_version() -> Option<Version> {
    if cfg!(target_env = "musl") {
        // Dynamically loading a library is not supported on musl so we have to fall-back to using
        // the nvidia-smi command.
        detect_cuda_version_via_nvidia_smi()
    } else {
        detect_cuda_version_via_nvml().or_else(|| {
            tracing::debug!(
                "NVML did not detect a CUDA driver version; trying libcuda/nvidia-smi fallbacks"
            );
            detect_cuda_version_fallbacks().map(|(version, _source)| version)
        })
    }
}

fn detect_cuda_version_fallbacks() -> Option<(Version, CudaDetectionMethod)> {
    if cfg!(target_env = "musl") {
        return detect_cuda_version_via_nvidia_smi()
            .map(|version| (version, CudaDetectionMethod::NvidiaSmi));
    }

    let version = detect_cuda_version_via_libcuda();
    if let Some(version) = version {
        return Some((version, CudaDetectionMethod::Libcuda));
    }

    tracing::debug!("libcuda did not detect a CUDA driver version; trying nvidia-smi fallback");
    detect_cuda_version_via_nvidia_smi().map(|version| (version, CudaDetectionMethod::NvidiaSmi))
}

/// Attempts to detect the version of CUDA present in the current operating system by loading the
/// NVIDIA Management Library and querying the CUDA driver version. The method is preferred over
/// [`detect_cuda_version_via_libcuda`] because that method might fail base on environment
/// variables.
///
/// Although the required methods in the runtime are not implemented on much older machines it is
/// considered old enough to be usable for our use case. Since Conda doesn't provide old versions of
/// the CUDA SDK anyway this is considered a non-issue.
///
/// Some drivers can answer `nvmlSystemGetCudaDriverVersion` without `nvmlInit`, avoiding the
/// expensive driver handshake / GPU attach that makes `nvmlInit` slow on Windows. If that no-init
/// query fails, this falls back to querying while NVML is initialized.
pub fn detect_cuda_version_via_nvml() -> Option<Version> {
    // Try to open the library
    let mut library = None;
    for path in nvml_library_paths() {
        match unsafe { libloading::Library::new(*path) } {
            Ok(loaded) => {
                tracing::trace!(library_path = *path, "loaded NVML library");
                library = Some(loaded);
                break;
            }
            Err(err) => {
                tracing::trace!(library_path = *path, error = %err, "failed to load NVML library");
            }
        }
    }
    let library = library?;

    match cuda_version_from_nvml_library(&library) {
        Ok(version) => {
            tracing::trace!(%version, "detected CUDA driver version via NVML without init");
            Some(version)
        }
        Err(err) if err.should_retry_after_init() => {
            tracing::debug!(?err, "retrying CUDA driver version query after nvmlInit");
            detect_cuda_initialized_info_via_nvml(&library, true, false).0
        }
        Err(err) => {
            tracing::debug!(?err, "CUDA driver version query via NVML failed");
            None
        }
    }
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
    let mut cuda_library = None;
    for path in cuda_library_paths() {
        match unsafe { libloading::Library::new(*path) } {
            Ok(loaded) => {
                tracing::trace!(library_path = *path, "loaded CUDA driver library");
                cuda_library = Some(loaded);
                break;
            }
            Err(err) => {
                tracing::trace!(
                    library_path = *path,
                    error = %err,
                    "failed to load CUDA driver library"
                );
            }
        }
    }
    let cuda_library = cuda_library?;

    // Get entry points from the library. `CUresult` is a 32-bit enum, so these are declared to
    // return `c_int` (matching the NVML declarations); on some ABIs the upper bits of a wider
    // return register are unspecified.
    let cu_init: Symbol<'_, unsafe extern "C" fn(c_uint) -> c_int> =
        match unsafe { cuda_library.get(b"cuInit\0") } {
            Ok(symbol) => symbol,
            Err(err) => {
                tracing::debug!(error = %err, "missing cuInit symbol");
                return None;
            }
        };
    let cu_driver_get_version: Symbol<'_, unsafe extern "C" fn(*mut c_int) -> c_int> =
        match unsafe { cuda_library.get(b"cuDriverGetVersion\0") } {
            Ok(symbol) => symbol,
            Err(err) => {
                tracing::debug!(error = %err, "missing cuDriverGetVersion symbol");
                return None;
            }
        };

    // Initialize the CUDA library
    let init_result = unsafe { cu_init(0) };
    if init_result != 0 {
        tracing::debug!(return_code = init_result, "cuInit failed");
        return None;
    }

    // Get the version from the library. The out-parameter is zero-initialized so that a driver
    // returning success without writing it yields a deterministic `0`, which is rejected below.
    let mut version_int: c_int = 0;
    let version_result = unsafe { cu_driver_get_version(&mut version_int) };
    if version_result != 0 {
        tracing::debug!(return_code = version_result, "cuDriverGetVersion failed");
        return None;
    }

    // Convert the version integer to a version string
    let version = parse_cuda_driver_version(version_int);
    if let Some(version) = &version {
        tracing::trace!(%version, "detected CUDA driver version via libcuda");
    } else {
        tracing::trace!("failed to parse CUDA driver version reported by libcuda");
    }
    version
}

/// Attempts to detect the CUDA compute capability by loading the CUDA driver library and
/// enumerating all devices, returning the **minimum** compute capability across all devices.
///
/// This is the fallback for platforms that ship libcuda but neither NVML nor nvidia-smi (e.g.
/// Jetson/Tegra). Device enumeration through libcuda is affected by `CUDA_VISIBLE_DEVICES`, so
/// the NVML and nvidia-smi probes are preferred.
fn detect_cuda_arch_via_libcuda() -> Option<CudaArchInfo> {
    // CUDA device attribute constants for querying compute capability
    const CU_DEVICE_ATTRIBUTE_COMPUTE_CAPABILITY_MAJOR: c_int = 75;
    const CU_DEVICE_ATTRIBUTE_COMPUTE_CAPABILITY_MINOR: c_int = 76;

    // Try to open the library
    let mut cuda_library = None;
    for path in cuda_library_paths() {
        match unsafe { libloading::Library::new(*path) } {
            Ok(loaded) => {
                tracing::trace!(library_path = *path, "loaded CUDA driver library");
                cuda_library = Some(loaded);
                break;
            }
            Err(err) => {
                tracing::trace!(
                    library_path = *path,
                    error = %err,
                    "failed to load CUDA driver library"
                );
            }
        }
    }
    let cuda_library = cuda_library?;

    // Get entry points from the library. `CUresult` is a 32-bit enum, so these are declared to
    // return `c_int`.
    let cu_init: Symbol<'_, unsafe extern "C" fn(c_uint) -> c_int> =
        match unsafe { cuda_library.get(b"cuInit\0") } {
            Ok(symbol) => symbol,
            Err(err) => {
                tracing::debug!(error = %err, "missing cuInit symbol");
                return None;
            }
        };
    let cu_device_get_count: Symbol<'_, unsafe extern "C" fn(*mut c_int) -> c_int> =
        match unsafe { cuda_library.get(b"cuDeviceGetCount\0") } {
            Ok(symbol) => symbol,
            Err(err) => {
                tracing::debug!(error = %err, "missing cuDeviceGetCount symbol");
                return None;
            }
        };
    let cu_device_get: Symbol<'_, unsafe extern "C" fn(*mut c_int, c_int) -> c_int> =
        match unsafe { cuda_library.get(b"cuDeviceGet\0") } {
            Ok(symbol) => symbol,
            Err(err) => {
                tracing::debug!(error = %err, "missing cuDeviceGet symbol");
                return None;
            }
        };
    let cu_device_get_attribute: Symbol<
        '_,
        unsafe extern "C" fn(*mut c_int, c_int, c_int) -> c_int,
    > = match unsafe { cuda_library.get(b"cuDeviceGetAttribute\0") } {
        Ok(symbol) => symbol,
        Err(err) => {
            tracing::debug!(error = %err, "missing cuDeviceGetAttribute symbol");
            return None;
        }
    };

    // Initialize the CUDA library
    let init_result = unsafe { cu_init(0) };
    if init_result != 0 {
        tracing::debug!(return_code = init_result, "cuInit failed");
        return None;
    }

    // Get the number of CUDA devices
    let mut device_count: c_int = 0;
    let device_count_result = unsafe { cu_device_get_count(&mut device_count) };
    if device_count_result != 0 {
        tracing::trace!(return_code = device_count_result, "cuDeviceGetCount failed");
        return None;
    }
    tracing::trace!(device_count, "enumerating CUDA devices via libcuda");

    // Iterate through all devices to find the minimum compute capability
    let mut min_arch: Option<CudaArchInfo> = None;
    for device_idx in 0..device_count {
        let mut device: c_int = 0;
        let device_result = unsafe { cu_device_get(&mut device, device_idx) };
        if device_result != 0 {
            tracing::trace!(
                device_idx,
                return_code = device_result,
                "failed to get CUDA device handle"
            );
            continue;
        }

        let mut cc_major: c_int = 0;
        let mut cc_minor: c_int = 0;
        let major_result = unsafe {
            cu_device_get_attribute(
                &mut cc_major,
                CU_DEVICE_ATTRIBUTE_COMPUTE_CAPABILITY_MAJOR,
                device,
            )
        };
        let minor_result = unsafe {
            cu_device_get_attribute(
                &mut cc_minor,
                CU_DEVICE_ATTRIBUTE_COMPUTE_CAPABILITY_MINOR,
                device,
            )
        };
        if major_result != 0 || minor_result != 0 {
            tracing::trace!(
                device_idx,
                major_return_code = major_result,
                minor_return_code = minor_result,
                "failed to get CUDA compute capability via libcuda"
            );
            continue;
        }
        let cc_major = cc_major as u32;
        let cc_minor = cc_minor as u32;
        tracing::trace!(
            device_idx,
            major = cc_major,
            minor = cc_minor,
            "detected CUDA compute capability via libcuda"
        );

        let is_new_minimum = min_arch.as_ref().is_none_or(|min| {
            cc_major < min.major || (cc_major == min.major && cc_minor < min.minor)
        });
        if is_new_minimum {
            min_arch = Some(CudaArchInfo {
                major: cc_major,
                minor: cc_minor,
            });
        }
    }

    if let Some(arch) = min_arch.as_ref() {
        tracing::debug!(
            major = arch.major,
            minor = arch.minor,
            "selected minimum CUDA compute capability from libcuda"
        );
    } else {
        tracing::debug!("no CUDA compute capability detected via libcuda");
    }

    min_arch
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
    tracing::trace!("detecting CUDA driver version via nvidia-smi");
    // Invoke the "nvidia-smi" command to query the driver version that is usually installed when
    // Cuda drivers are installed.
    let nvidia_smi_output = match Command::new("nvidia-smi")
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
    {
        Ok(output) => output,
        Err(err) => {
            tracing::debug!(error = %err, "failed to run nvidia-smi for CUDA driver version");
            return None;
        }
    };

    // nvidia-smi can exit non-zero in degraded-but-parseable states (e.g. one GPU lost while others
    // are healthy) where the XML still contains a usable `<cuda_version>`. Log the failure but still
    // attempt to parse stdout instead of bailing out on the exit status.
    if !nvidia_smi_output.status.success() {
        tracing::debug!(
            status = %nvidia_smi_output.status,
            stderr = %String::from_utf8_lossy(&nvidia_smi_output.stderr),
            "nvidia-smi CUDA driver version query exited non-zero; attempting to parse output anyway"
        );
    }

    // Convert the output to Utf8. The conversion is lossy so it might contain some illegal
    // characters. If that is the case we simply assume the version in the file also wont make sense
    // during parsing.
    let output = String::from_utf8_lossy(&nvidia_smi_output.stdout);
    parse_nvidia_smi_cuda_version(&output)
}

/// Extracts the CUDA driver version from the XML output produced by `nvidia-smi --query -u -x`.
///
/// Returns `None` if the `<cuda_version>` element is missing or cannot be parsed as a [`Version`].
fn parse_nvidia_smi_cuda_version(output: &str) -> Option<Version> {
    static CUDA_VERSION_RE: once_cell::sync::Lazy<regex::Regex> =
        once_cell::sync::Lazy::new(|| {
            regex::Regex::new("<cuda_version>(.*)<\\/cuda_version>").unwrap()
        });

    // Extract the version from the XML
    let Some(version_match) = CUDA_VERSION_RE.captures(output) else {
        tracing::trace!("nvidia-smi output did not contain a CUDA driver version");
        return None;
    };
    let Some(version_match) = version_match.get(1) else {
        tracing::trace!("nvidia-smi CUDA driver version match was empty");
        return None;
    };
    let version_str = version_match.as_str();

    // Parse and return
    match Version::from_str(version_str) {
        Ok(version) => {
            tracing::trace!(%version, "detected CUDA driver version via nvidia-smi");
            Some(version)
        }
        Err(err) => {
            tracing::trace!(version = version_str, error = %err, "failed to parse nvidia-smi CUDA driver version");
            None
        }
    }
}

/// Attempts to detect the CUDA compute capability by executing the "nvidia-smi" command and
/// querying the `compute_cap` field of every GPU, returning the **minimum** across all devices.
///
/// Like [`detect_cuda_version_via_nvidia_smi`] this does not dynamically load a library and thus
/// also works on musl systems. The `compute_cap` query field requires a reasonably modern driver
/// (roughly R510+); on older drivers the command fails and `None` is returned.
fn detect_cuda_arch_via_nvidia_smi() -> Option<CudaArchInfo> {
    tracing::trace!("detecting CUDA compute capability via nvidia-smi");
    let nvidia_smi_output = match Command::new("nvidia-smi")
        // Query the compute capability of every GPU as plain CSV, one line per GPU.
        .arg("--query-gpu=compute_cap")
        .arg("--format=csv,noheader")
        // See `detect_cuda_version_via_nvidia_smi` for why this variable is removed.
        .env_remove("CUDA_VISIBLE_DEVICES")
        .output()
    {
        Ok(output) => output,
        Err(err) => {
            tracing::debug!(error = %err, "failed to run nvidia-smi for CUDA compute capability");
            return None;
        }
    };

    // On drivers that do not support the `compute_cap` field the command exits with an error, but it
    // can also exit non-zero while still reporting some healthy GPUs on stdout. Log the failure but
    // still attempt to parse whatever was produced instead of bailing out on the exit status.
    if !nvidia_smi_output.status.success() {
        tracing::debug!(
            status = %nvidia_smi_output.status,
            stderr = %String::from_utf8_lossy(&nvidia_smi_output.stderr),
            "nvidia-smi CUDA compute capability query exited non-zero; attempting to parse output anyway"
        );
    }

    let output = String::from_utf8_lossy(&nvidia_smi_output.stdout);
    parse_nvidia_smi_compute_capabilities(&output)
}

/// Parses the CSV output of `nvidia-smi --query-gpu=compute_cap --format=csv,noheader` and returns
/// the **minimum** compute capability across all parseable GPU lines.
///
/// Each line is expected to be a `major.minor` value. Lines that are not in that format (such as
/// `[N/A]` reported for a GPU whose capability is unknown, or other junk) are skipped while still
/// using the valid ones. Returns `None` if no line could be parsed.
fn parse_nvidia_smi_compute_capabilities(output: &str) -> Option<CudaArchInfo> {
    // Find the minimum compute capability across all devices
    let mut min_arch: Option<CudaArchInfo> = None;
    for (device_idx, line) in output.lines().enumerate() {
        let line = line.trim();
        let Some((major, minor)) = line.split_once('.') else {
            tracing::trace!(
                device_idx,
                line,
                "ignoring invalid nvidia-smi compute capability line"
            );
            continue;
        };
        let (Ok(major), Ok(minor)) = (major.parse::<u32>(), minor.parse::<u32>()) else {
            tracing::trace!(
                device_idx,
                line,
                "ignoring unparsable nvidia-smi compute capability line"
            );
            continue;
        };
        tracing::trace!(
            device_idx,
            major,
            minor,
            "detected CUDA compute capability via nvidia-smi"
        );

        let is_new_minimum = min_arch
            .as_ref()
            .is_none_or(|min| major < min.major || (major == min.major && minor < min.minor));
        if is_new_minimum {
            min_arch = Some(CudaArchInfo { major, minor });
        }
    }

    if let Some(arch) = min_arch.as_ref() {
        tracing::debug!(
            major = arch.major,
            minor = arch.minor,
            "selected minimum CUDA compute capability from nvidia-smi"
        );
    } else {
        tracing::debug!("no CUDA compute capability detected via nvidia-smi");
    }

    min_arch
}

#[cfg(test)]
mod test {
    use super::*;

    /// Times loading the NVML library only, as the first NVML use in this process.
    ///
    /// Compared against [`bench_cold_version`] this separates the cost of mapping the library from
    /// the cost of the version query itself (which loads the CUDA driver library internally).
    ///
    /// Run on its own, see [`bench_cold_version`].
    #[test]
    #[ignore = "benchmark, run manually and in isolation"]
    fn bench_cold_library_load() {
        let start = std::time::Instant::now();
        let loaded = nvml_library_paths()
            .iter()
            .find_map(|path| unsafe { Library::new(*path).ok() })
            .is_some();
        println!(
            "cold NVML library load:     {:?}  -> loaded={loaded}",
            start.elapsed()
        );
    }

    /// Times a single cold `__cuda` detection, as the first NVML call in this process.
    ///
    /// The dominant factor is how long ago anything last touched the GPU, not the code path: the
    /// driver powers down when idle, and the first query after that re-initializes it. Measured on
    /// Windows with an NVIDIA GPU this is around 1.5s for an idle driver against roughly 460ms for
    /// one that was used seconds earlier, so compare runs in the same driver state.
    ///
    /// Run on its own so nothing else has warmed up the driver:
    ///
    /// ```text
    /// cargo test -p rattler_virtual_packages --release -- --ignored --nocapture --exact \
    ///     cuda::test::bench_cold_version
    /// ```
    #[test]
    #[ignore = "benchmark, run manually and in isolation"]
    fn bench_cold_version() {
        let start = std::time::Instant::now();
        let version = detect_cuda_version_via_nvml();
        println!(
            "cold __cuda (no init):      {:?}  -> {version:?}",
            start.elapsed()
        );
    }

    /// Times a single cold `__cuda_arch` detection, as the first NVML call in this process.
    ///
    /// Run on its own, see [`bench_cold_version`].
    #[test]
    #[ignore = "benchmark, run manually and in isolation"]
    fn bench_cold_arch() {
        let start = std::time::Instant::now();
        let info = detect_cuda_info();
        println!(
            "cold __cuda + __cuda_arch:  {:?}  -> {:?}",
            start.elapsed(),
            info.info.arch_info
        );
    }

    /// Times a single cold detection through the on-disk cache, as the first NVML call in this
    /// process. Run twice: the first run populates the cache, the second measures a cache hit.
    ///
    /// Run on its own, see [`bench_cold_version`].
    #[test]
    #[ignore = "benchmark, run manually and in isolation"]
    fn bench_cold_cached() {
        let cache_dir = std::env::temp_dir().join("rattler-cuda-bench-cache");
        std::fs::create_dir_all(&cache_dir).unwrap();
        let start = std::time::Instant::now();
        let info = cuda_info(Some(&cache_dir));
        println!(
            "cold detection via cache:   {:?}  -> {:?} / {:?}",
            start.elapsed(),
            info.version,
            info.arch_info
        );
        println!(
            "(run again to measure a cache hit; cache dir: {})",
            cache_dir.display()
        );
    }

    /// Times each CUDA detection path so the cost of the on-disk cache can be
    /// compared against actually talking to the driver.
    ///
    /// Ignored by default because it is a benchmark and because the interesting
    /// numbers only show up on a machine with an NVIDIA GPU. Run it with:
    ///
    /// ```text
    /// cargo test -p rattler_virtual_packages --release -- --ignored --nocapture bench_cuda_detection
    /// ```
    #[test]
    #[ignore = "benchmark, run manually on a machine with an NVIDIA GPU"]
    fn bench_cuda_detection() {
        /// Reproduction of the detection this crate used before the driver library was queried
        /// without initializing NVML, so the two can be compared side by side.
        fn legacy_detect_cuda_version_via_nvml() -> Option<Version> {
            let library = nvml_library_paths()
                .iter()
                .find_map(|path| unsafe { Library::new(*path).ok() })?;
            let nvml_init: Symbol<'_, unsafe extern "C" fn() -> c_int> = unsafe {
                library
                    .get(b"nvmlInit_v2\0")
                    .or_else(|_| library.get(b"nvmlInit\0"))
            }
            .ok()?;
            let nvml_shutdown: Symbol<'_, unsafe extern "C" fn() -> c_int> =
                unsafe { library.get(b"nvmlShutdown\0") }.ok()?;
            let get_version: Symbol<'_, unsafe extern "C" fn(*mut c_int) -> c_int> = unsafe {
                library
                    .get(b"nvmlSystemGetCudaDriverVersion_v2\0")
                    .or_else(|_| library.get(b"nvmlSystemGetCudaDriverVersion\0"))
            }
            .ok()?;

            if unsafe { nvml_init() } != 0 {
                return None;
            }
            let mut raw = std::mem::MaybeUninit::uninit();
            let result = unsafe { get_version(raw.as_mut_ptr()) };
            let _ = unsafe { nvml_shutdown() };
            if result != 0 {
                return None;
            }
            parse_cuda_driver_version(unsafe { raw.assume_init() })
        }

        // The first call is the one that matters: every process pays it once, and the on-disk
        // cache exists to avoid exactly that. The warm best-of-5 is reported next to it to show
        // how much of the cost is one-time (library loading, driver wake-up) rather than
        // intrinsic to the query.
        fn time<T>(label: &str, mut f: impl FnMut() -> T) -> T {
            let start = std::time::Instant::now();
            let mut result = f();
            let first = start.elapsed();

            let mut best = std::time::Duration::MAX;
            for _ in 0..5 {
                let start = std::time::Instant::now();
                result = f();
                best = best.min(start.elapsed());
            }
            println!("{label:<48} {first:>12.3?} {best:>12.3?}");
            result
        }

        println!("\n{:<48} {:>12} {:>12}", "", "first (cold)", "best of 5");
        println!("--- detection paths ---");
        let legacy = time("version via NVML (legacy, with init)", || {
            legacy_detect_cuda_version_via_nvml()
        });
        let version = time("version via NVML (no init)", detect_cuda_version_via_nvml);
        let info = time("version + arch via NVML (one init)", detect_cuda_info);
        let smi_version = time("version via nvidia-smi", detect_cuda_version_via_nvidia_smi);
        let smi_arch = time("arch via nvidia-smi", detect_cuda_arch_via_nvidia_smi);
        println!("legacy version:   {legacy:?}");

        println!("\n--- breakdown ---");
        if let Some(path) = nvml_library_paths()
            .iter()
            .copied()
            .find(|path| unsafe { Library::new(*path) }.is_ok())
        {
            time("load NVML library only", || {
                drop(unsafe { Library::new(path) });
            });
            let library = unsafe { Library::new(path) }.expect("just loaded successfully");
            time("version query (library already loaded)", || {
                cuda_version_from_nvml_library(&library).ok()
            });
            time("init + arch (library already loaded)", || {
                detect_cuda_initialized_info_via_nvml(&library, false, true)
            });
        }
        if let Some(path) = cuda_library_paths()
            .iter()
            .copied()
            .find(|path| unsafe { Library::new(*path) }.is_ok())
        {
            time("load CUDA driver library only", || {
                drop(unsafe { Library::new(path) });
            });
        }

        println!("\n--- cache paths ---");
        let dir = tempfile::tempdir().unwrap();
        let env = time("cache env (boot + driver + device keys)", || {
            cache::CacheEnv::current()
        });
        time("cache write", || {
            cache::write_with_env(&env, dir.path(), &info.info, info.sources);
        });
        let cache_read = time("cache read (hit)", || {
            cache::read_with_env(&env, dir.path())
        });

        println!("\n--- results ---");
        println!("version:          {version:?}");
        println!("arch:             {:?}", info.info.arch_info);
        println!("nvidia-smi:       {smi_version:?} / {smi_arch:?}");
        println!("cache read:       {:?}", cache_read.map(|c| c.info));
        if version.is_none() {
            println!(
                "\nNOTE: no CUDA driver found, so the driver paths short-circuit and their\n\
                 timings are meaningless. Run this on a machine with an NVIDIA GPU."
            );
        }
    }

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
    pub fn doesnt_crash_nvidia_smi_arch() {
        let arch = detect_cuda_arch_via_nvidia_smi();
        println!("Cuda arch {arch:?}");
    }

    #[test]
    pub fn test_cuda_info() {
        let info = cuda_info(None);
        println!("CUDA Info: {info:?}");
        if let Some(ref arch) = info.arch_info {
            println!("  Compute capability: {}.{}", arch.major, arch.minor);
        }
    }

    #[test]
    pub fn test_cuda_arch() {
        let arch = cuda_arch(None);
        println!("CUDA Arch: {arch:?}");
    }

    /// Builds a fully specified, deterministic cache environment for the tests. The `driver`
    /// string becomes a kernel-module fingerprint and the `device` string a single GPU bus id.
    fn fake_env(
        boot: &str,
        driver: Option<&str>,
        device: Option<&str>,
        now: u64,
    ) -> cache::CacheEnv {
        cache::CacheEnv {
            boot_id: Some(cache::BootId::Uuid(boot.to_owned())),
            driver_fingerprint: driver.map(|version| cache::DriverFingerprint::Module {
                version: version.to_owned(),
            }),
            device_fingerprint: device.map(|gpu| cache::DeviceFingerprint {
                gpus: vec![gpu.to_owned()],
                device_nodes: Vec::new(),
            }),
            now,
        }
    }

    fn full_info() -> (CudaInfo, CudaInfoSources) {
        (
            CudaInfo {
                version: Some(Version::from_str("12.4").unwrap()),
                arch_info: Some(CudaArchInfo { major: 8, minor: 6 }),
            },
            CudaInfoSources {
                version: Some(CudaDetectionMethod::NvmlInitialized),
                arch: Some(CudaDetectionMethod::NvidiaSmi),
            },
        )
    }

    #[test]
    fn test_cache_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let env = fake_env("boot-1", Some("driver-1"), Some("dev-1"), 1_000);

        // Nothing cached yet.
        assert!(cache::read_with_env(&env, dir.path()).is_none());

        // Negative results are not cached.
        cache::write_with_env(
            &env,
            dir.path(),
            &CudaInfo {
                version: None,
                arch_info: None,
            },
            CudaInfoSources::default(),
        );
        assert!(cache::read_with_env(&env, dir.path()).is_none());

        let (info, sources) = full_info();
        cache::write_with_env(&env, dir.path(), &info, sources);
        let cached = cache::read_with_env(&env, dir.path()).unwrap();
        assert_eq!(cached.info.version, info.version);
        assert_eq!(cached.info.arch_info, info.arch_info);
        assert_eq!(cached.sources, sources);

        // Updating the cache should atomically replace the previous file.
        let updated_info = CudaInfo {
            version: Some(Version::from_str("12.5").unwrap()),
            arch_info: None,
        };
        let updated_sources = CudaInfoSources {
            version: Some(CudaDetectionMethod::Libcuda),
            arch: None,
        };
        cache::write_with_env(&env, dir.path(), &updated_info, updated_sources);
        let cached = cache::read_with_env(&env, dir.path()).unwrap();
        assert_eq!(cached.info.version, updated_info.version);
        assert_eq!(cached.info.arch_info, updated_info.arch_info);
        assert_eq!(cached.sources, updated_sources);
    }

    #[test]
    fn test_cache_ttl_version_only_vs_full() {
        let dir = tempfile::tempdir().unwrap();
        let write_env = fake_env("boot-1", Some("driver-1"), Some("dev-1"), 1_000);

        // A version-only entry (transient arch failure) is readable immediately but expires after
        // ten minutes.
        let version_only = CudaInfo {
            version: Some(Version::from_str("12.4").unwrap()),
            arch_info: None,
        };
        cache::write_with_env(
            &write_env,
            dir.path(),
            &version_only,
            CudaInfoSources::default(),
        );
        assert!(cache::read_with_env(&write_env, dir.path()).is_some());
        let just_before = fake_env("boot-1", Some("driver-1"), Some("dev-1"), 1_000 + 600);
        assert!(cache::read_with_env(&just_before, dir.path()).is_some());
        let after_10m = fake_env("boot-1", Some("driver-1"), Some("dev-1"), 1_000 + 601);
        assert!(cache::read_with_env(&after_10m, dir.path()).is_none());

        // A full entry is still readable at that same age (it uses the 24h TTL) but expires past a
        // day.
        let (info, sources) = full_info();
        cache::write_with_env(&write_env, dir.path(), &info, sources);
        assert!(cache::read_with_env(&after_10m, dir.path()).is_some());
        let after_24h = fake_env(
            "boot-1",
            Some("driver-1"),
            Some("dev-1"),
            1_000 + 24 * 3600 + 1,
        );
        assert!(cache::read_with_env(&after_24h, dir.path()).is_none());
    }

    #[test]
    fn test_cache_rejects_future_write_time() {
        let dir = tempfile::tempdir().unwrap();
        let write_env = fake_env("boot-1", Some("driver-1"), Some("dev-1"), 10_000);
        let (info, sources) = full_info();
        cache::write_with_env(&write_env, dir.path(), &info, sources);

        // Written more than five minutes in the future (clock stepped backwards): rejected.
        let past = fake_env("boot-1", Some("driver-1"), Some("dev-1"), 10_000 - 301);
        assert!(cache::read_with_env(&past, dir.path()).is_none());
        // Within the tolerated skew: still accepted.
        let slight_past = fake_env("boot-1", Some("driver-1"), Some("dev-1"), 10_000 - 299);
        assert!(cache::read_with_env(&slight_past, dir.path()).is_some());
    }

    #[test]
    fn test_cache_invalidated_on_device_change() {
        let dir = tempfile::tempdir().unwrap();
        let env = fake_env("boot-1", Some("driver-1"), Some("dev-a"), 1_000);
        let (info, sources) = full_info();
        cache::write_with_env(&env, dir.path(), &info, sources);

        // A different device fingerprint (e.g. another container / hot-plugged GPU) invalidates.
        let other_device = fake_env("boot-1", Some("driver-1"), Some("dev-b"), 1_000);
        assert!(cache::read_with_env(&other_device, dir.path()).is_none());
        // `Some` cached versus `None` current also invalidates.
        let no_device = fake_env("boot-1", Some("driver-1"), None, 1_000);
        assert!(cache::read_with_env(&no_device, dir.path()).is_none());
        // The matching fingerprint still reads.
        assert!(cache::read_with_env(&env, dir.path()).is_some());
    }

    #[test]
    fn test_cache_requires_driver_fingerprint() {
        let dir = tempfile::tempdir().unwrap();
        let (info, sources) = full_info();

        // Without a current driver fingerprint nothing is written.
        let no_driver = fake_env("boot-1", None, Some("dev-1"), 1_000);
        cache::write_with_env(&no_driver, dir.path(), &info, sources);
        assert!(!dir.path().join("cuda-info-v1.json").exists());

        // A hand-written cache file is rejected when the current fingerprint is unavailable.
        std::fs::write(
            dir.path().join("cuda-info-v1.json"),
            r#"{"boot_id":{"uuid":"boot-1"},"driver_fingerprint":{"module":{"version":"driver-1"}},"device_fingerprint":{"gpus":["dev-1"],"device_nodes":[]},"written_at":1000,"version":"12.4","arch":[8,6]}"#,
        )
        .unwrap();
        assert!(cache::read_with_env(&no_driver, dir.path()).is_none());
        // With a driver fingerprint available it reads.
        let with_driver = fake_env("boot-1", Some("driver-1"), Some("dev-1"), 1_000);
        assert!(cache::read_with_env(&with_driver, dir.path()).is_some());
    }

    #[test]
    fn test_cache_invalidated_after_driver_change() {
        let dir = tempfile::tempdir().unwrap();
        // A cache file written with a different driver installed is ignored.
        std::fs::write(
            dir.path().join("cuda-info-v1.json"),
            r#"{"boot_id":{"uuid":"boot-1"},"driver_fingerprint":{"module":{"version":"535.0"}},"device_fingerprint":null,"written_at":1000,"version":"12.4","arch":[8,6]}"#,
        )
        .unwrap();
        let stale = fake_env("boot-1", Some("550.0"), None, 1_000);
        assert!(cache::read_with_env(&stale, dir.path()).is_none());
        // The original driver still reads.
        let current = fake_env("boot-1", Some("535.0"), None, 1_000);
        assert!(cache::read_with_env(&current, dir.path()).is_some());
    }

    #[test]
    fn test_cache_invalidated_after_reboot() {
        let dir = tempfile::tempdir().unwrap();
        // A cache file from a different boot session is ignored.
        std::fs::write(
            dir.path().join("cuda-info-v1.json"),
            r#"{"boot_id":{"uuid":"boot-A"},"driver_fingerprint":{"module":{"version":"driver-1"}},"device_fingerprint":null,"written_at":1000,"version":"12.4","arch":[8,6]}"#,
        )
        .unwrap();
        let other_boot = fake_env("boot-B", Some("driver-1"), None, 1_000);
        assert!(cache::read_with_env(&other_boot, dir.path()).is_none());
        // The same boot session still reads.
        let same_boot = fake_env("boot-A", Some("driver-1"), None, 1_000);
        assert!(cache::read_with_env(&same_boot, dir.path()).is_some());
    }

    #[test]
    fn test_boot_time_tolerance() {
        // The extracted numeric comparison, testable on every platform.
        assert!(cache::boot_times_within_tolerance(1_000, 1_000));
        assert!(cache::boot_times_within_tolerance(1_000, 1_120));
        assert!(cache::boot_times_within_tolerance(1_120, 1_000));
        assert!(!cache::boot_times_within_tolerance(1_000, 1_121));

        // Two derived boot times match within tolerance.
        let a = cache::BootId::BootTime(1000);
        let b = cache::BootId::BootTime(1050);
        assert!(a.matches(&b));
        let c = cache::BootId::BootTime(2000);
        assert!(!a.matches(&c));

        // Boot counters must match exactly, and a boot counter never matches a boot time.
        let count = cache::BootId::BootCount(5);
        assert!(count.matches(&cache::BootId::BootCount(5)));
        assert!(!count.matches(&cache::BootId::BootCount(6)));
        assert!(!count.matches(&a));
    }

    #[test]
    fn test_late_persistence() {
        let dir = tempfile::tempdir().unwrap();
        let env = fake_env("boot-1", Some("driver-1"), Some("dev-1"), 1_000);

        // Pre-seed the state with a known detection result, as if detection had already run.
        let state: OnceCell<DetectedCudaInfo> = OnceCell::new();
        let (info, sources) = full_info();
        let _ = state.set(DetectedCudaInfo { info, sources });
        let persisted = AtomicBool::new(false);

        // A call with no cache directory does not persist to disk.
        cuda_info_impl(&env, &state, &persisted, None);
        assert!(!persisted.load(Ordering::Relaxed));
        assert!(cache::read_with_env(&env, dir.path()).is_none());

        // A later call with a cache directory persists the already-detected result.
        cuda_info_impl(&env, &state, &persisted, Some(dir.path()));
        assert!(persisted.load(Ordering::Relaxed));
        let cached = cache::read_with_env(&env, dir.path()).unwrap();
        assert_eq!(
            cached.info.version,
            Some(Version::from_str("12.4").unwrap())
        );
        assert_eq!(
            cached.info.arch_info,
            Some(CudaArchInfo { major: 8, minor: 6 })
        );
    }

    #[test]
    fn test_is_nvidia_pci_device_id() {
        // Real NVIDIA device ids as Windows reports them
        assert!(cache::is_nvidia_pci_device_id(
            "PCI\\VEN_10DE&DEV_2484&SUBSYS_147D10DE&REV_A1\\4&2D2E5D1F&0&0008"
        ));
        assert!(cache::is_nvidia_pci_device_id("PCI\\VEN_10DE&DEV_1EB1"));
        // Match case insensitively
        assert!(cache::is_nvidia_pci_device_id("pci\\ven_10de&dev_2484"));

        // Other vendors: Intel, AMD
        assert!(!cache::is_nvidia_pci_device_id(
            "PCI\\VEN_8086&DEV_9A49&SUBSYS_00011025&REV_01\\3&11583659&0&10"
        ));
        assert!(!cache::is_nvidia_pci_device_id("PCI\\VEN_1002&DEV_73FF"));
        assert!(!cache::is_nvidia_pci_device_id(""));
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
    fn test_parse_cuda_driver_version() {
        // Valid values are decoded as `major.minor`.
        assert_eq!(
            parse_cuda_driver_version(12_040),
            Some(Version::from_str("12.4").unwrap())
        );
        assert_eq!(
            parse_cuda_driver_version(11_080),
            Some(Version::from_str("11.8").unwrap())
        );
        // Smallest plausible value (CUDA major 1).
        assert_eq!(
            parse_cuda_driver_version(1_000),
            Some(Version::from_str("1.0").unwrap())
        );
        // Largest plausible value (CUDA major 99).
        assert_eq!(
            parse_cuda_driver_version(99_990),
            Some(Version::from_str("99.99").unwrap())
        );

        // Implausible values are rejected rather than propagating garbage.
        assert_eq!(parse_cuda_driver_version(0), None);
        assert_eq!(parse_cuda_driver_version(-1), None);
        assert_eq!(parse_cuda_driver_version(999), None);
        assert_eq!(parse_cuda_driver_version(100_000), None);
        assert_eq!(parse_cuda_driver_version(c_int::MAX), None);
        assert_eq!(parse_cuda_driver_version(c_int::MIN), None);
    }

    #[test]
    fn test_parse_nvidia_smi_cuda_version() {
        // A representative fragment of the `nvidia-smi --query -u -x` XML output.
        let xml = "<nvidia_smi_log>\n  <cuda_version>12.4</cuda_version>\n</nvidia_smi_log>";
        assert_eq!(
            parse_nvidia_smi_cuda_version(xml),
            Some(Version::from_str("12.4").unwrap())
        );

        // Missing the tag entirely.
        assert_eq!(
            parse_nvidia_smi_cuda_version("<nvidia_smi_log></nvidia_smi_log>"),
            None
        );

        // Empty input.
        assert_eq!(parse_nvidia_smi_cuda_version(""), None);
    }

    #[test]
    fn test_parse_nvidia_smi_compute_capabilities() {
        // Multiple GPUs: the minimum capability is selected.
        assert_eq!(
            parse_nvidia_smi_compute_capabilities("8.6\n7.5\n9.0"),
            Some(CudaArchInfo { major: 7, minor: 5 })
        );

        // Junk and `[N/A]`-style lines are skipped while the valid ones are still used. Here the
        // valid minimum line (7.5) is retained even though another GPU reports `[N/A]`.
        assert_eq!(
            parse_nvidia_smi_compute_capabilities("8.6\n[N/A]\n7.5\ngarbage"),
            Some(CudaArchInfo { major: 7, minor: 5 })
        );

        // A line that is unparsable as numbers (`x.y`) is skipped as well.
        assert_eq!(
            parse_nvidia_smi_compute_capabilities("x.y\n8.0"),
            Some(CudaArchInfo { major: 8, minor: 0 })
        );

        // All lines are junk: nothing is detected.
        assert_eq!(
            parse_nvidia_smi_compute_capabilities("[N/A]\ngarbage\n"),
            None
        );

        // Empty input.
        assert_eq!(parse_nvidia_smi_compute_capabilities(""), None);
    }
}
