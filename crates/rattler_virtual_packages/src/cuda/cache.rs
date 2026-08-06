//! On-disk cache for detected CUDA information, valid for the current boot session.
//!
//! Detecting CUDA can be slow (initializing NVML attaches to every GPU), so the result is cached
//! on disk between processes. The cache is keyed on everything that can invalidate a previous
//! detection without the code noticing:
//!
//! * the current **boot session**, because a reboot re-enumerates hardware and drivers;
//! * a **driver fingerprint**, because drivers can be updated (and NVML reloaded) without a reboot;
//! * a **device fingerprint** of the host-visible GPUs, because two containers sharing a cache
//!   volume see different GPU subsets and topology can change within a session (eGPU hot-plug,
//!   PCI hot-plug into VMs, suspend/resume);
//! * a **TTL** as a staleness backstop for anything the fingerprints cannot catch (and to let
//!   transient arch-detection failures self-heal quickly).
//!
//! Reads and writes are best-effort: any failure simply results in a fresh detection. Delete the
//! file or use the `CONDA_OVERRIDE_CUDA*` variables to bypass it.

use super::{CudaArchInfo, CudaDetectionMethod, CudaInfo, CudaInfoSources, DetectedCudaInfo};
use rattler_conda_types::Version;
use serde::{Deserialize, Serialize};
use std::{
    io::Write,
    path::{Path, PathBuf},
    str::FromStr,
};

const CACHE_FILE_NAME: &str = "cuda-info-v1.json";

/// Full detections (with compute capability) are trusted for a day; the fingerprints catch most
/// changes sooner, so this is only a staleness backstop.
const FULL_TTL_SECS: u64 = 24 * 60 * 60;
/// Entries without compute capability represent a transient arch-detection failure, so they expire
/// quickly and self-heal instead of lingering for the whole boot session.
const ARCH_MISSING_TTL_SECS: u64 = 10 * 60;
/// Reject entries whose write time is further than this into the future, which means the clock
/// stepped backwards and the recorded `written_at` can no longer be trusted for the TTL.
const MAX_CLOCK_SKEW_SECS: u64 = 5 * 60;

/// Identifies a single boot session of the machine.
///
/// All variants are compiled on every platform so the comparison logic can be unit-tested
/// anywhere; `current` only ever produces the variants that exist on the host platform.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum BootId {
    /// The kernel's per-boot UUID from `/proc/sys/kernel/random/boot_id` (Linux).
    Uuid(String),
    /// The prefetcher boot counter from the registry, incremented once per boot (Windows).
    BootCount(u32),
    /// A boot time in unix seconds derived from the uptime (Windows fallback). The derivation
    /// drifts a little between processes, which `matches` absorbs with a tolerance.
    BootTime(u64),
}

impl BootId {
    /// Returns the identifier of the current boot session, or `None` if it cannot be determined
    /// (in which case no caching takes place).
    pub(super) fn current() -> Option<Self> {
        #[cfg(target_os = "linux")]
        {
            // The kernel generates a fresh UUID on every boot.
            let id = std::fs::read_to_string("/proc/sys/kernel/random/boot_id").ok()?;
            Some(Self::Uuid(id.trim().to_owned()))
        }
        #[cfg(target_os = "windows")]
        {
            // Prefer the prefetcher boot counter: it increments exactly once per boot and involves
            // no clock arithmetic, so it cannot be confused by reboots or clock steps.
            if let Some(count) = windows_boot_count() {
                return Some(Self::BootCount(count));
            }
            // Fall back to deriving the boot time from the uptime.
            let uptime_secs =
                unsafe { windows_sys::Win32::System::SystemInformation::GetTickCount64() } / 1000;
            let now_secs = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .ok()?
                .as_secs();
            Some(Self::BootTime(now_secs.checked_sub(uptime_secs)?))
        }
        #[cfg(not(any(target_os = "linux", target_os = "windows")))]
        {
            None
        }
    }

    /// Returns true if both identifiers refer to the same boot session.
    pub(super) fn matches(&self, other: &Self) -> bool {
        match (self, other) {
            // Two derived boot times can drift a few seconds between processes; treat them as the
            // same session when they are within tolerance.
            (Self::BootTime(a), Self::BootTime(b)) => boot_times_within_tolerance(*a, *b),
            // Everything else (boot UUIDs, boot counters, or mixed kinds) must match exactly; two
            // different kinds never refer to the same session.
            _ => self == other,
        }
    }
}

/// Returns true if two `boottime:` second values are close enough to be the same boot session.
///
/// Extracted as a plain function (not `cfg(windows)`-gated) so the tolerance logic is compiled and
/// unit-tested on every platform. A real reboot shifts the derived boot time by at least the
/// previous uptime, which is far larger than this tolerance.
pub(super) fn boot_times_within_tolerance(a: u64, b: u64) -> bool {
    /// The derived boot time drifts a little between processes.
    const BOOT_TIME_TOLERANCE_SECS: u64 = 120;
    a.abs_diff(b) <= BOOT_TIME_TOLERANCE_SECS
}

/// Reads the prefetcher boot counter from the registry, incremented once per boot.
#[cfg(target_os = "windows")]
fn windows_boot_count() -> Option<u32> {
    use windows_sys::Win32::System::Registry::{
        HKEY_LOCAL_MACHINE, RRF_RT_REG_DWORD, RegGetValueW,
    };

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(std::iter::once(0)).collect()
    }

    let subkey = wide(
        "SYSTEM\\CurrentControlSet\\Control\\Session Manager\\Memory Management\\PrefetchParameters",
    );
    let value = wide("BootId");
    let mut data: u32 = 0;
    let mut data_size: u32 = std::mem::size_of::<u32>() as u32;
    let status = unsafe {
        RegGetValueW(
            HKEY_LOCAL_MACHINE,
            subkey.as_ptr(),
            value.as_ptr(),
            RRF_RT_REG_DWORD,
            std::ptr::null_mut(),
            std::ptr::addr_of_mut!(data).cast::<std::ffi::c_void>(),
            &mut data_size,
        )
    };
    // ERROR_SUCCESS
    if status == 0 { Some(data) } else { None }
}

/// Identifies the installed NVIDIA driver.
///
/// Drivers can be updated without a reboot, so the boot session alone is not enough to key the
/// cache on. All variants are compiled on every platform so they can be unit-tested anywhere.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum DriverFingerprint {
    /// The version of the loaded `nvidia` kernel module (Linux), which changes when the driver is
    /// updated and the module is reloaded.
    Module { version: String },
    /// The identity of the NVML library file on disk (Windows, WSL2), which driver updates
    /// replace.
    File {
        path: PathBuf,
        mtime_secs: u64,
        len: u64,
    },
}

/// Identifies the set of host-visible GPUs.
///
/// This distinguishes containers that share a cache volume but see different GPU subsets, and it
/// changes when GPUs are hot-plugged or unplugged within a boot session.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(super) struct DeviceFingerprint {
    /// Sorted identifiers of the GPUs the host exposes: PCI bus ids from
    /// `/proc/driver/nvidia/gpus` on Linux, Plug and Play device ids on Windows.
    pub(super) gpus: Vec<String>,
    /// Sorted `/dev/nvidiaN` device nodes that are present. Always empty on Windows, which has no
    /// device nodes.
    pub(super) device_nodes: Vec<String>,
}

#[derive(Serialize, Deserialize)]
struct CacheFile {
    /// The boot session during which detection ran.
    boot_id: BootId,
    /// Fingerprint of the installed driver when detection ran. Required: entries without a driver
    /// fingerprint cannot be trusted, so they are neither written nor read.
    driver_fingerprint: DriverFingerprint,
    /// Fingerprint of the host-visible GPUs when detection ran, or `None` if it could not be
    /// determined on this platform.
    device_fingerprint: Option<DeviceFingerprint>,
    /// Unix seconds at which the entry was written, used for the TTL.
    written_at: u64,
    version: String,
    arch: Option<(u32, u32)>,
    #[serde(default)]
    version_source: Option<CudaDetectionMethod>,
    #[serde(default)]
    arch_source: Option<CudaDetectionMethod>,
}

/// The environment against which a cache entry is validated: everything that can invalidate a
/// previous detection, plus the current time for the TTL.
///
/// [`read_with_env`] and [`write_with_env`] operate against an explicit `CacheEnv` so they can be
/// unit-tested deterministically on any machine; [`CacheEnv::current`] gathers the real values for
/// the call sites in `cuda.rs`.
pub(super) struct CacheEnv {
    pub(super) boot_id: Option<BootId>,
    pub(super) driver_fingerprint: Option<DriverFingerprint>,
    pub(super) device_fingerprint: Option<DeviceFingerprint>,
    pub(super) now: u64,
}

impl CacheEnv {
    /// Gathers the real cache environment from the current host.
    pub(super) fn current() -> Self {
        Self {
            boot_id: BootId::current(),
            driver_fingerprint: driver_fingerprint(),
            device_fingerprint: device_fingerprint(),
            now: now_unix_secs(),
        }
    }
}

/// Returns the current time in unix seconds, or `0` if the clock is before the epoch.
fn now_unix_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

/// Absolute libnvidia-ml paths probed by the detector, used as a driver-fingerprint fallback where
/// `/sys/module/nvidia` does not exist (notably WSL2).
///
/// Keep in sync with the absolute entries of `nvml_library_paths()` in `cuda.rs`.
#[cfg(target_os = "linux")]
const LIBNVIDIA_ML_ABSOLUTE_PATHS: &[&str] = &[
    "/usr/lib64/nvidia/libnvidia-ml.so.1", // RHEL/Centos/Fedora
    "/usr/lib64/nvidia/libnvidia-ml.so",
    "/usr/lib/x86_64-linux-gnu/libnvidia-ml.so.1", // Ubuntu
    "/usr/lib/x86_64-linux-gnu/libnvidia-ml.so",
    "/usr/lib/wsl/lib/libnvidia-ml.so.1", // WSL
    "/usr/lib/wsl/lib/libnvidia-ml.so",
];

/// Fingerprints a file by its path, modification time and length, or `None` if it does not exist.
#[cfg(any(target_os = "linux", target_os = "windows"))]
fn file_fingerprint(path: &Path) -> Option<DriverFingerprint> {
    let metadata = std::fs::metadata(path).ok()?;
    let mtime = metadata
        .modified()
        .ok()?
        .duration_since(std::time::UNIX_EPOCH)
        .ok()?;
    Some(DriverFingerprint::File {
        path: path.to_path_buf(),
        mtime_secs: mtime.as_secs(),
        len: metadata.len(),
    })
}

/// Returns a fingerprint of the installed NVIDIA driver, or `None` if it cannot be determined.
fn driver_fingerprint() -> Option<DriverFingerprint> {
    #[cfg(target_os = "linux")]
    {
        // The version of the loaded kernel module, which changes when the driver is updated and
        // the module is reloaded.
        if let Ok(version) = std::fs::read_to_string("/sys/module/nvidia/version") {
            return Some(DriverFingerprint::Module {
                version: version.trim().to_owned(),
            });
        }
        // WSL2 (and similar setups) has no `/sys/module/nvidia`, so fall back to fingerprinting the
        // libnvidia-ml file the detector would load.
        for path in LIBNVIDIA_ML_ABSOLUTE_PATHS {
            if let Some(fingerprint) = file_fingerprint(Path::new(path)) {
                return Some(fingerprint);
            }
        }
        None
    }
    #[cfg(target_os = "windows")]
    {
        // Driver updates on Windows usually complete without a reboot but replace nvml.dll. The DLL
        // does not always load from System32, so also consider the NVSMI install location.
        let mut candidates = Vec::new();
        if let Some(windir) = std::env::var_os("WINDIR") {
            candidates.push(Path::new(&windir).join("System32").join("nvml.dll"));
        }
        if let Some(program_files) = std::env::var_os("ProgramFiles") {
            candidates.push(
                Path::new(&program_files)
                    .join("NVIDIA Corporation")
                    .join("NVSMI")
                    .join("nvml.dll"),
            );
        }
        for path in candidates {
            if let Some(fingerprint) = file_fingerprint(&path) {
                return Some(fingerprint);
            }
        }
        None
    }
    #[cfg(not(any(target_os = "linux", target_os = "windows")))]
    {
        None
    }
}

/// Returns true if a `/dev` entry name is an `nvidiaN` device node (all-digits suffix), which
/// excludes control nodes like `nvidiactl` and `nvidia-uvm`.
#[cfg(target_os = "linux")]
fn is_nvidia_device_node(name: &str) -> bool {
    name.strip_prefix("nvidia")
        .is_some_and(|rest| !rest.is_empty() && rest.bytes().all(|b| b.is_ascii_digit()))
}

/// Returns a fingerprint of the host-visible GPUs, or `None` if it cannot be determined.
fn device_fingerprint() -> Option<DeviceFingerprint> {
    #[cfg(target_os = "linux")]
    {
        // PCI bus ids of the GPUs the driver exposes to us.
        let gpus_dir = std::fs::read_dir("/proc/driver/nvidia/gpus").ok();
        let has_gpus_dir = gpus_dir.is_some();
        let mut gpus: Vec<String> = gpus_dir
            .map(|entries| {
                entries
                    .filter_map(Result::ok)
                    .map(|entry| entry.file_name().to_string_lossy().into_owned())
                    .collect()
            })
            .unwrap_or_default();
        gpus.sort();

        // The `/dev/nvidiaN` device nodes that are actually present.
        let mut devices: Vec<String> = std::fs::read_dir("/dev")
            .map(|entries| {
                entries
                    .filter_map(Result::ok)
                    .map(|entry| entry.file_name().to_string_lossy().into_owned())
                    .filter(|name| is_nvidia_device_node(name))
                    .collect()
            })
            .unwrap_or_default();
        devices.sort();

        if !has_gpus_dir && devices.is_empty() {
            return None;
        }
        Some(DeviceFingerprint {
            gpus,
            device_nodes: devices,
        })
    }
    #[cfg(target_os = "windows")]
    {
        // Enumerate the NVIDIA PCI devices Windows knows about. This only reads Plug and Play metadata, so
        // it does not initialize the driver or wake a powered-down GPU.
        let mut gpus = windows_nvidia_device_ids()?;
        gpus.sort();
        Some(DeviceFingerprint {
            gpus,
            // Device nodes are a Linux concept.
            device_nodes: Vec::new(),
        })
    }
    #[cfg(not(any(target_os = "linux", target_os = "windows")))]
    {
        // No cheap per-container GPU enumeration is available; the TTL is the backstop here.
        None
    }
}

/// Returns the Plug and Play device ids of all NVIDIA PCI devices, or `None` if they cannot be
/// enumerated.
///
/// Uses the configuration manager rather than the driver so that hot-plugged GPUs (for instance an
/// external GPU over Thunderbolt) are noticed without paying for driver initialization.
#[cfg(target_os = "windows")]
fn windows_nvidia_device_ids() -> Option<Vec<String>> {
    use windows_sys::Win32::Devices::DeviceAndDriverInstallation::{
        CM_GETIDLIST_FILTER_ENUMERATOR, CM_GETIDLIST_FILTER_PRESENT, CM_Get_Device_ID_List_SizeW,
        CM_Get_Device_ID_ListW, CR_SUCCESS,
    };

    // Only enumerate devices that are currently present on the PCI bus.
    let filter: Vec<u16> = "PCI".encode_utf16().chain(std::iter::once(0)).collect();
    let flags = CM_GETIDLIST_FILTER_ENUMERATOR | CM_GETIDLIST_FILTER_PRESENT;

    let mut len: u32 = 0;
    if unsafe { CM_Get_Device_ID_List_SizeW(&mut len, filter.as_ptr(), flags) } != CR_SUCCESS {
        return None;
    }

    let mut buffer = vec![0u16; len as usize];
    if unsafe { CM_Get_Device_ID_ListW(filter.as_ptr(), buffer.as_mut_ptr(), len, flags) }
        != CR_SUCCESS
    {
        return None;
    }

    // The buffer is a sequence of null terminated strings, terminated by an empty string.
    Some(
        buffer
            .split(|&c| c == 0)
            .filter(|segment| !segment.is_empty())
            .map(String::from_utf16_lossy)
            .filter(|id| is_nvidia_pci_device_id(id))
            .collect(),
    )
}

/// Returns true if `id` is the Plug and Play device id of an NVIDIA PCI device.
///
/// NVIDIA's PCI vendor id is `10DE`. Windows reports device ids uppercased, but match
/// case-insensitively to be safe.
#[cfg(any(target_os = "windows", test))]
pub(super) fn is_nvidia_pci_device_id(id: &str) -> bool {
    id.to_ascii_uppercase().contains("VEN_10DE")
}

/// Reads a cached detection result, validating it against the given host environment.
pub(super) fn read_with_env(env: &CacheEnv, cache_dir: &Path) -> Option<DetectedCudaInfo> {
    let path = cache_dir.join(CACHE_FILE_NAME);
    let Ok(content) = std::fs::read_to_string(&path) else {
        tracing::trace!("no CUDA info cache found at {}", path.display());
        return None;
    };
    let Ok(cached) = serde_json::from_str::<CacheFile>(&content) else {
        tracing::debug!("ignoring invalid CUDA info cache at {}", path.display());
        return None;
    };
    let Some(current_boot_id) = env.boot_id.as_ref() else {
        tracing::debug!(
            "ignoring CUDA info cache because the current boot id could not be determined"
        );
        return None;
    };
    if !cached.boot_id.matches(current_boot_id) {
        tracing::info!(
            cache_path = %path.display(),
            cached_boot_id = ?cached.boot_id,
            current_boot_id = ?current_boot_id,
            "invalidating CUDA info cache from a previous boot session"
        );
        return None;
    }
    let Some(current_driver_fingerprint) = env.driver_fingerprint.as_ref() else {
        tracing::debug!(
            "ignoring CUDA info cache because the current driver fingerprint could not be determined"
        );
        return None;
    };
    if &cached.driver_fingerprint != current_driver_fingerprint {
        tracing::info!(
            cache_path = %path.display(),
            cached_driver_fingerprint = ?cached.driver_fingerprint,
            current_driver_fingerprint = ?current_driver_fingerprint,
            "invalidating CUDA info cache because the driver changed"
        );
        return None;
    }
    if cached.device_fingerprint != env.device_fingerprint {
        tracing::info!(
            cache_path = %path.display(),
            cached_device_fingerprint = ?cached.device_fingerprint,
            current_device_fingerprint = ?env.device_fingerprint,
            "invalidating CUDA info cache because the visible GPUs changed"
        );
        return None;
    }
    // Reject entries written in the future: the clock stepped backwards and the TTL below can no
    // longer be trusted.
    if cached.written_at.saturating_sub(env.now) > MAX_CLOCK_SKEW_SECS {
        tracing::debug!(
            cache_path = %path.display(),
            written_at = cached.written_at,
            now = env.now,
            "ignoring CUDA info cache written in the future"
        );
        return None;
    }
    // Full detections are trusted for a day; transient arch failures self-heal within minutes.
    let ttl = if cached.arch.is_some() {
        FULL_TTL_SECS
    } else {
        ARCH_MISSING_TTL_SECS
    };
    if env.now.saturating_sub(cached.written_at) > ttl {
        tracing::info!(
            cache_path = %path.display(),
            written_at = cached.written_at,
            now = env.now,
            ttl,
            "invalidating expired CUDA info cache"
        );
        return None;
    }
    let version = match Version::from_str(&cached.version) {
        Ok(version) => version,
        Err(err) => {
            tracing::debug!(
                version = cached.version,
                error = %err,
                "ignoring CUDA info cache with invalid version"
            );
            return None;
        }
    };
    tracing::trace!("using CUDA info cached at {}", path.display());
    Some(DetectedCudaInfo {
        info: CudaInfo {
            version: Some(version),
            arch_info: cached
                .arch
                .map(|(major, minor)| CudaArchInfo { major, minor }),
        },
        sources: CudaInfoSources {
            version: cached.version_source,
            arch: cached.arch_source,
        },
    })
}

/// Writes a detection result to the cache, keyed on the given host environment.
pub(super) fn write_with_env(
    env: &CacheEnv,
    cache_dir: &Path,
    info: &CudaInfo,
    sources: CudaInfoSources,
) {
    // Only cache when a driver was found: detection without a driver is fast anyway, and not
    // caching the negative result means a freshly installed driver is picked up immediately.
    let Some(version) = &info.version else {
        tracing::trace!("not caching CUDA info because no CUDA driver version was detected");
        return;
    };
    let Some(boot_id) = env.boot_id.clone() else {
        tracing::debug!(
            "not caching CUDA info because the current boot id could not be determined"
        );
        return;
    };
    // The driver fingerprint is required: without it the cache is keyed on the boot session alone
    // (a host driver update would serve stale data on e.g. WSL2), so we simply do not cache.
    let Some(driver_fingerprint) = env.driver_fingerprint.clone() else {
        tracing::debug!(
            "not caching CUDA info because the current driver fingerprint could not be determined"
        );
        return;
    };
    if let Err(err) = std::fs::create_dir_all(cache_dir) {
        tracing::debug!(
            cache_dir = %cache_dir.display(),
            error = %err,
            "failed to create CUDA info cache directory"
        );
        return;
    }
    let cached = CacheFile {
        boot_id,
        driver_fingerprint,
        device_fingerprint: env.device_fingerprint.clone(),
        written_at: env.now,
        version: version.to_string(),
        arch: info.arch_info.as_ref().map(|arch| (arch.major, arch.minor)),
        version_source: sources.version,
        arch_source: sources.arch,
    };
    let Ok(content) = serde_json::to_string(&cached) else {
        tracing::debug!("failed to serialize CUDA info cache entry");
        return;
    };
    // Write to a temporary file in the cache directory and persist it into place so concurrent
    // readers never see a partial cache file.
    let path = cache_dir.join(CACHE_FILE_NAME);
    let mut tmp = match tempfile::NamedTempFile::new_in(cache_dir) {
        Ok(tmp) => tmp,
        Err(err) => {
            tracing::debug!(
                cache_dir = %cache_dir.display(),
                error = %err,
                "failed to create temporary CUDA info cache file"
            );
            return;
        }
    };
    if let Err(err) = tmp.write_all(content.as_bytes()) {
        tracing::debug!(
            cache_path = %path.display(),
            error = %err,
            "failed to write temporary CUDA info cache file"
        );
        return;
    }
    match tmp.persist(&path) {
        Ok(_) => tracing::trace!("cached CUDA info at {}", path.display()),
        Err(err) => {
            tracing::debug!(
                cache_path = %path.display(),
                error = %err.error,
                "failed to persist CUDA info cache"
            );
        }
    }
}
