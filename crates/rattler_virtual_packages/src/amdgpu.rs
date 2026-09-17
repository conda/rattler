//! Provides functionality to detect AMD GPUs present on the current system.
//!
//! This module backs two virtual packages, as specified by the `__amdgpu` CEP
//! (<https://github.com/conda/ceps/pull/189>):
//!
//! ## AMD GPU presence (`__amdgpu`)
//!
//! A presence-only package (version and build string are always `0`) that is exposed when at least
//! one AMD GPU is available through the host driver.
//!
//! ## AMD GPU architecture (`__amdgpu_arch`)
//!
//! The AMDGPU ISA version (`{major}.{minor}.{stepping}`) of the detected GPU with the most compute
//! units; ties are broken by picking the highest ISA version. Unlike `__cuda_arch` this is *not* a
//! minimum: AMDGPU binaries carry code objects for specific `gfx` targets and there is no
//! forward-compatibility model like PTX, so the most capable device is the most useful one to
//! describe.
//!
//! ## Detection
//!
//! * Linux: the KFD topology in `/sys/class/kfd/kfd/topology/nodes` provides the ISA version and
//!   compute-unit count per device; `/sys/class/drm` is consulted for presence when KFD is not
//!   available (e.g. a display-only driver setup). When KFD exposes no devices the HIP runtime is
//!   tried as a fallback, which covers `ROCm` on WSL where the KFD sysfs tree does not exist.
//! * Windows: the HIP runtime (`amdhip64_<major>.dll` / `amdhip64.dll`) installed by the AMD
//!   driver in the system directory provides the ISA version and compute-unit count. Presence is
//!   additionally derived from the AMD display adapters Windows knows about.

use std::{fmt, str::FromStr, sync::LazyLock};

use rattler_conda_types::Version;

/// The AMDGPU ISA version of a device, e.g. `9.0.10` for `gfx90a` or `11.5.1` for `gfx1151`.
///
/// The derived ordering is lexicographic over `(major, minor, stepping)`, which is the ordering the
/// CEP prescribes for breaking compute-unit ties. It does *not* imply binary compatibility between
/// architectures.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct AmdGpuArchInfo {
    /// Major version, e.g. `11` for `gfx1151`.
    pub major: u32,
    /// Minor version, e.g. `5` for `gfx1151`.
    pub minor: u32,
    /// Stepping, e.g. `1` for `gfx1151` or `10` for `gfx90a`.
    pub stepping: u32,
}

impl AmdGpuArchInfo {
    /// Decodes the `gfx_target_version` property exposed by the Linux KFD topology.
    ///
    /// The value is encoded as `major * 10000 + minor * 100 + stepping`, e.g. `90010` for `gfx90a`.
    /// A value of `0` marks a node that is not a GPU (KFD lists CPU nodes as well) and yields `None`.
    pub fn from_gfx_target_version(value: u32) -> Option<Self> {
        if value == 0 {
            return None;
        }
        Some(Self {
            major: (value / 10000) % 100,
            minor: (value / 100) % 100,
            stepping: value % 100,
        })
    }

    /// Decodes an AMDGPU target id as reported by HIP's `gcnArchName`, e.g. `gfx90a` or
    /// `gfx1100:xnack-`.
    ///
    /// Any target feature suffix after the first `:` is ignored. After the `gfx` prefix, the final
    /// two characters are the hexadecimal minor and stepping and everything before them is the
    /// decimal major version.
    pub fn from_target_id(target_id: &str) -> Option<Self> {
        let name = target_id.split(':').next().unwrap_or_default();
        let digits = name.strip_prefix("gfx")?;
        if digits.len() < 3 {
            return None;
        }
        let (major, rest) = digits.split_at(digits.len() - 2);
        let mut rest = rest.chars();
        let minor = rest.next()?.to_digit(16)?;
        let stepping = rest.next()?.to_digit(16)?;
        if !major.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
        Some(Self {
            major: major.parse().ok()?,
            minor,
            stepping,
        })
    }

    /// Converts the ISA version into the [`Version`] used for the `__amdgpu_arch` virtual package.
    pub fn to_version(self) -> Version {
        Version::from_str(&self.to_string())
            .expect("three dot separated decimal integers are a valid version")
    }
}

impl fmt::Display for AmdGpuArchInfo {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}.{}", self.major, self.minor, self.stepping)
    }
}

/// An error returned when parsing an AMDGPU ISA version from a string.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error(
    "invalid AMDGPU ISA version '{0}': expected 'major.minor.stepping' where all three are decimal integers (e.g. '11.0.0')"
)]
pub struct ParseAmdGpuArchError(String);

impl FromStr for AmdGpuArchInfo {
    type Err = ParseAmdGpuArchError;

    /// Parses the `{major}.{minor}.{stepping}` form used by the `__amdgpu_arch` virtual package
    /// and its `CONDA_OVERRIDE_AMDGPU_ARCH` override.
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let invalid = || ParseAmdGpuArchError(s.to_string());
        let mut parts = s.split('.');
        let mut component = || {
            let part = parts.next().ok_or_else(invalid)?;
            if part.is_empty() || !part.bytes().all(|b| b.is_ascii_digit()) {
                return Err(invalid());
            }
            part.parse::<u32>().map_err(|_overflow| invalid())
        };
        let major = component()?;
        let minor = component()?;
        let stepping = component()?;
        if parts.next().is_some() {
            return Err(invalid());
        }
        Ok(Self {
            major,
            minor,
            stepping,
        })
    }
}

/// A single detected AMD GPU.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AmdGpuDevice {
    /// The ISA version of the device.
    pub arch: AmdGpuArchInfo,
    /// The number of compute units, if it could be determined.
    pub compute_units: Option<u32>,
    /// The device's marketing name (`hipDeviceProp_t::name`), e.g. `"AMD Radeon RX 7800 XT"`.
    ///
    /// Only set for devices enumerated through the HIP runtime; the Linux KFD topology sysfs
    /// this module also reads from has no equivalent, only the raw PCI device id.
    pub name: Option<String>,
}

impl fmt::Display for AmdGpuDevice {
    /// Renders as e.g. `"AMD Radeon RX 7800 XT (arch 11.0.1, 30 CUs)"`, or, when the marketing
    /// name or compute-unit count is unavailable (only possible on the Linux KFD path), `"AMD
    /// GPU (arch 11.0.1, CU count unknown)"`.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.name.as_deref().unwrap_or("AMD GPU"))?;
        write!(f, " (arch {}", self.arch)?;
        match self.compute_units {
            Some(1) => write!(f, ", 1 CU)"),
            Some(compute_units) => write!(f, ", {compute_units} CUs)"),
            None => write!(f, ", CU count unknown)"),
        }
    }
}

/// AMD GPU information detected from the system.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AmdGpuInfo {
    /// Whether at least one AMD GPU is available through the host driver. This corresponds to the
    /// `__amdgpu` virtual package.
    pub present: bool,

    /// The ISA version selected for the `__amdgpu_arch` virtual package: the device with the most
    /// compute units, ties broken by the highest ISA version. `None` when no device could be
    /// enumerated, which also happens for some presence-only detections (e.g. a Linux system whose
    /// driver exposes the GPU through DRM but not through KFD).
    ///
    /// Always `None` when `present` is `false`.
    pub arch_info: Option<AmdGpuArchInfo>,
}

/// Selects the device that represents the system in `__amdgpu_arch`: the one with the most compute
/// units, ties (and unknown counts, which sort below any known count) broken by the highest ISA
/// version.
pub fn select_device(devices: &[AmdGpuDevice]) -> Option<&AmdGpuDevice> {
    devices
        .iter()
        .max_by_key(|device| (device.compute_units, device.arch))
}

/// Returns the AMD GPU information of the current platform.
///
/// Detection runs at most once per process; the result is reused afterwards.
pub fn amdgpu_info() -> &'static AmdGpuInfo {
    static DETECTED: LazyLock<AmdGpuInfo> = LazyLock::new(|| {
        let info = detect_amdgpu_info();
        tracing::trace!(
            present = info.present,
            arch = %info.arch_info.map_or_else(|| "<none>".to_string(), |arch| arch.to_string()),
            "detected AMD GPU info from host"
        );
        info
    });
    &DETECTED
}

/// Builds the final [`AmdGpuInfo`] from the enumerated devices and a separate presence signal.
fn info_from_devices(devices: &[AmdGpuDevice], present_without_devices: bool) -> AmdGpuInfo {
    let selected = select_device(devices);
    if let Some(device) = selected {
        tracing::debug!(
            %device,
            device_count = devices.len(),
            "selected AMD GPU for __amdgpu_arch"
        );
    }
    AmdGpuInfo {
        present: selected.is_some() || present_without_devices,
        arch_info: selected.map(|device| device.arch),
    }
}

/// Detects AMD GPU information from the current system.
#[cfg(target_os = "linux")]
pub fn detect_amdgpu_info() -> AmdGpuInfo {
    use std::path::Path;

    let mut devices = linux::kfd_devices(Path::new(linux::KFD_TOPOLOGY_NODES));
    if devices.is_empty() {
        devices = linux_hip_fallback();
    }
    let drm_present = devices.is_empty() && linux::drm_has_amdgpu(Path::new(linux::DRM_CLASS));
    info_from_devices(&devices, drm_present)
}

/// Falls back to HIP runtime enumeration when KFD exposes no devices, which covers `ROCm` on WSL
/// where the KFD sysfs tree does not exist.
#[cfg(all(target_os = "linux", not(target_env = "musl")))]
fn linux_hip_fallback() -> Vec<AmdGpuDevice> {
    tracing::trace!("KFD exposes no AMD GPUs; trying the HIP runtime");
    hip::devices()
}

/// Dynamic library loading is not available on musl: `libloading` itself would compile, but the
/// statically-linked binaries this target produces have no dynamic loader to `dlopen` a driver
/// library with, so the HIP runtime can never actually be reached. Skip compiling it altogether.
#[cfg(all(target_os = "linux", target_env = "musl"))]
fn linux_hip_fallback() -> Vec<AmdGpuDevice> {
    tracing::trace!("KFD exposes no AMD GPUs and musl cannot load the HIP runtime");
    Vec::new()
}

/// Detects AMD GPU information from the current system.
#[cfg(target_os = "windows")]
pub fn detect_amdgpu_info() -> AmdGpuInfo {
    let devices = hip::devices();
    let adapter_present = devices.is_empty() && windows::has_amd_display_adapter();
    info_from_devices(&devices, adapter_present)
}

/// Detects AMD GPU information from the current system.
///
/// There is no AMDGPU host driver on this platform, so nothing is ever detected.
#[cfg(not(any(target_os = "linux", target_os = "windows")))]
pub fn detect_amdgpu_info() -> AmdGpuInfo {
    info_from_devices(&[], false)
}

#[cfg(target_os = "linux")]
mod linux {
    use std::path::Path;

    use super::{AmdGpuArchInfo, AmdGpuDevice};

    /// The KFD topology; one directory per node with a `properties` file.
    pub(super) const KFD_TOPOLOGY_NODES: &str = "/sys/class/kfd/kfd/topology/nodes";

    /// The DRM class; one `cardN` directory per display adapter.
    pub(super) const DRM_CLASS: &str = "/sys/class/drm";

    /// Enumerates the GPUs in the KFD topology rooted at `nodes`.
    ///
    /// Every node has a `properties` file with `<key> <value>` lines. GPU nodes have a non-zero
    /// `gfx_target_version`; the compute-unit count is `simd_count / simd_per_cu`.
    pub(super) fn kfd_devices(nodes: &Path) -> Vec<AmdGpuDevice> {
        let Ok(entries) = std::fs::read_dir(nodes) else {
            tracing::trace!(path = %nodes.display(), "KFD topology is not available");
            return Vec::new();
        };
        let mut devices: Vec<_> = entries
            .filter_map(Result::ok)
            .filter_map(|entry| {
                let path = entry.path().join("properties");
                let properties = std::fs::read_to_string(&path).ok()?;
                let device = parse_kfd_properties(&properties)?;
                tracing::trace!(path = %path.display(), %device, "found AMD GPU in KFD topology");
                Some(device)
            })
            .collect();
        // `read_dir` order is arbitrary; keep the result deterministic.
        devices.sort_by_key(|device| (device.arch, device.compute_units));
        devices
    }

    /// Parses the `properties` file of a KFD topology node. Returns `None` for non-GPU nodes.
    pub(super) fn parse_kfd_properties(properties: &str) -> Option<AmdGpuDevice> {
        let mut gfx_target_version = None;
        let mut simd_count = None;
        let mut simd_per_cu = None;
        for line in properties.lines() {
            let mut fields = line.split_ascii_whitespace();
            let (Some(key), Some(value)) = (fields.next(), fields.next()) else {
                continue;
            };
            let slot = match key {
                "gfx_target_version" => &mut gfx_target_version,
                "simd_count" => &mut simd_count,
                "simd_per_cu" => &mut simd_per_cu,
                _ => continue,
            };
            *slot = value.parse::<u32>().ok();
        }
        let arch = AmdGpuArchInfo::from_gfx_target_version(gfx_target_version?)?;
        let compute_units = match (simd_count, simd_per_cu) {
            (Some(simd_count), Some(simd_per_cu)) if simd_per_cu > 0 => {
                Some(simd_count / simd_per_cu)
            }
            _ => None,
        };
        Some(AmdGpuDevice {
            arch,
            compute_units,
            name: None,
        })
    }

    /// Returns true if any DRM card in `drm` is an AMD device (PCI vendor `0x1002`) bound to the
    /// `amdgpu` kernel driver.
    pub(super) fn drm_has_amdgpu(drm: &Path) -> bool {
        let Ok(entries) = std::fs::read_dir(drm) else {
            return false;
        };
        entries.filter_map(Result::ok).any(|entry| {
            let name = entry.file_name();
            let Some(suffix) = name.to_str().and_then(|name| name.strip_prefix("card")) else {
                return false;
            };
            if suffix.is_empty() || !suffix.bytes().all(|b| b.is_ascii_digit()) {
                return false;
            }
            let device = entry.path().join("device");
            let is_amd = std::fs::read_to_string(device.join("vendor"))
                .is_ok_and(|vendor| vendor.trim().eq_ignore_ascii_case("0x1002"));
            let is_amdgpu = std::fs::read_link(device.join("driver"))
                .is_ok_and(|driver| driver.file_name().is_some_and(|name| name == "amdgpu"));
            if is_amd && is_amdgpu {
                tracing::trace!(path = %entry.path().display(), "found amdgpu DRM device");
            }
            is_amd && is_amdgpu
        })
    }
}

#[cfg(target_os = "windows")]
mod windows {
    use crate::win;

    /// Returns true if Windows knows about a present display adapter with AMD's PCI vendor id
    /// (`1002`). Only Plug and Play metadata is read, so this does not initialize the driver.
    pub(super) fn has_amd_display_adapter() -> bool {
        use windows_sys::Win32::Devices::DeviceAndDriverInstallation::{
            CM_GETIDLIST_FILTER_CLASS, CM_GETIDLIST_FILTER_PRESENT,
        };

        // GUID_DEVCLASS_DISPLAY
        const DISPLAY_CLASS: &str = "{4d36e968-e325-11ce-bfc1-08002be10318}";

        win::device_instance_ids(
            DISPLAY_CLASS,
            CM_GETIDLIST_FILTER_CLASS | CM_GETIDLIST_FILTER_PRESENT,
        )
        .is_some_and(|mut ids| ids.any(|id| is_amd_pci_device_id(&id)))
    }

    /// Returns true if `id` is the Plug and Play device id of an AMD PCI device.
    ///
    /// Windows reports device ids uppercased, but match case-insensitively to be safe.
    pub(super) fn is_amd_pci_device_id(id: &str) -> bool {
        id.to_ascii_uppercase().contains("VEN_1002")
    }
}

/// Device enumeration through the HIP runtime.
///
/// This is the only source of information on Windows, and the fallback on Linux when KFD is not
/// available. Not compiled on musl: `libloading` itself would compile fine, but the
/// statically-linked binaries that target produces have no dynamic loader to `dlopen` a driver
/// library with, so it could never actually be reached.
#[cfg(all(
    any(target_os = "linux", target_os = "windows"),
    not(target_env = "musl")
))]
mod hip {
    use std::ffi::{c_int, c_void};

    use libloading::{Library, Symbol};

    use super::{AmdGpuArchInfo, AmdGpuDevice};

    const HIP_SUCCESS: c_int = 0;

    /// The byte offsets of the `hipDeviceProp_t` members this module reads, per struct revision.
    ///
    /// The HIP runtime versions its ABI by exporting one `hipGetDeviceProperties*` symbol per
    /// struct revision. The struct is read from a zeroed, over-sized buffer at these offsets, so no
    /// full definition of the struct is needed.
    struct PropertiesLayout {
        symbol: &'static [u8],
        name: usize,
        gcn_arch_name: usize,
        multi_processor_count: usize,
    }

    /// Struct revisions in order of preference. Runtimes before HIP 6.0 only export the unversioned
    /// symbol, which uses the `R0000` layout.
    ///
    /// `name` is `hipDeviceProp_t`'s first member in every revision, so its offset is 0
    /// regardless of layout.
    const LAYOUTS: [PropertiesLayout; 3] = [
        PropertiesLayout {
            symbol: b"hipGetDevicePropertiesR0600\0",
            name: 0,
            gcn_arch_name: 1160,
            multi_processor_count: 388,
        },
        PropertiesLayout {
            symbol: b"hipGetDevicePropertiesR0000\0",
            name: 0,
            gcn_arch_name: 396,
            multi_processor_count: 336,
        },
        PropertiesLayout {
            symbol: b"hipGetDeviceProperties\0",
            name: 0,
            gcn_arch_name: 396,
            multi_processor_count: 336,
        },
    ];

    /// `name` and `gcnArchName` are both `char[256]`.
    const NAME_LEN: usize = 256;
    const GCN_ARCH_NAME_LEN: usize = 256;

    /// Comfortably larger than any `hipDeviceProp_t` revision (the `R0600` layout is 1480 bytes).
    const PROPERTIES_BUFFER_LEN: usize = 4096;

    type HipGetDeviceCount = unsafe extern "C" fn(*mut c_int) -> c_int;
    type HipGetDeviceProperties = unsafe extern "C" fn(*mut c_void, c_int) -> c_int;

    /// Enumerates the devices visible to the HIP runtime, or nothing if the runtime is not
    /// installed or reports no devices.
    pub(super) fn devices() -> Vec<AmdGpuDevice> {
        let Some(library) = load_library() else {
            tracing::debug!("could not load the HIP runtime from any known path");
            return Vec::new();
        };

        let get_count: Symbol<'_, HipGetDeviceCount> =
            match unsafe { library.get(b"hipGetDeviceCount\0") } {
                Ok(symbol) => symbol,
                Err(err) => {
                    tracing::debug!(error = %err, "HIP runtime lacks hipGetDeviceCount");
                    return Vec::new();
                }
            };
        let mut count: c_int = 0;
        let result = unsafe { get_count(&mut count) };
        if result != HIP_SUCCESS || count <= 0 {
            tracing::trace!(result, count, "HIP runtime reports no devices");
            return Vec::new();
        }

        let Some((get_properties, layout)) = LAYOUTS.iter().find_map(|layout| {
            let symbol: Symbol<'_, HipGetDeviceProperties> =
                unsafe { library.get(layout.symbol) }.ok()?;
            Some((symbol, layout))
        }) else {
            tracing::debug!("HIP runtime lacks every known hipGetDeviceProperties revision");
            return Vec::new();
        };

        (0..count)
            .filter_map(|device| {
                // `u64` storage keeps the buffer suitably aligned for every member.
                let mut buffer = [0u64; PROPERTIES_BUFFER_LEN / 8];
                let result =
                    unsafe { get_properties(buffer.as_mut_ptr().cast::<c_void>(), device) };
                if result != HIP_SUCCESS {
                    tracing::debug!(device, result, "hipGetDeviceProperties failed");
                    return None;
                }
                let bytes: &[u8] = unsafe {
                    std::slice::from_raw_parts(buffer.as_ptr().cast::<u8>(), PROPERTIES_BUFFER_LEN)
                };

                let target_id =
                    &bytes[layout.gcn_arch_name..layout.gcn_arch_name + GCN_ARCH_NAME_LEN];
                let target_id = &target_id[..target_id
                    .iter()
                    .position(|&b| b == 0)
                    .unwrap_or(target_id.len())];
                let target_id = std::str::from_utf8(target_id).ok()?;
                let Some(arch) = AmdGpuArchInfo::from_target_id(target_id) else {
                    tracing::debug!(device, target_id, "HIP reported an unparsable gcnArchName");
                    return None;
                };

                let count = &bytes[layout.multi_processor_count..layout.multi_processor_count + 4];
                let count = c_int::from_ne_bytes(count.try_into().expect("slice is 4 bytes"));
                let compute_units = u32::try_from(count).ok().filter(|&count| count > 0);

                // The marketing name is purely cosmetic: a non-UTF-8 or empty value just leaves
                // it unset rather than dropping the device.
                let name = &bytes[layout.name..layout.name + NAME_LEN];
                let name = &name[..name.iter().position(|&b| b == 0).unwrap_or(name.len())];
                let name = std::str::from_utf8(name)
                    .ok()
                    .filter(|name| !name.is_empty())
                    .map(str::to_owned);

                let device = AmdGpuDevice {
                    arch,
                    compute_units,
                    name,
                };
                tracing::debug!(target_id, %device, "found AMD GPU through the HIP runtime");
                Some(device)
            })
            .collect()
    }

    fn load_library() -> Option<Library> {
        library_paths().into_iter().find_map(|path| {
            match unsafe { Library::new(&path) } {
                Ok(library) => {
                    tracing::trace!(library_path = %path.display(), "loaded HIP runtime");
                    Some(library)
                }
                Err(err) => {
                    tracing::trace!(library_path = %path.display(), error = %err, "failed to load HIP runtime");
                    None
                }
            }
        })
    }

    /// The AMD driver installs the HIP runtime into the Windows system directory as
    /// `amdhip64_<major>.dll` (newest first) or, for older drivers, `amdhip64.dll`.
    #[cfg(target_os = "windows")]
    fn library_paths() -> Vec<std::path::PathBuf> {
        let system_root = std::env::var_os("SystemRoot").unwrap_or_else(|| r"C:\Windows".into());
        let system32 = std::path::Path::new(&system_root).join("System32");

        let mut versioned: Vec<(u32, std::path::PathBuf)> = std::fs::read_dir(&system32)
            .into_iter()
            .flatten()
            .filter_map(Result::ok)
            .filter_map(|entry| {
                let name = entry.file_name();
                let name = name.to_str()?.to_ascii_lowercase();
                let version = name
                    .strip_prefix("amdhip64_")?
                    .strip_suffix(".dll")?
                    .parse::<u32>()
                    .ok()?;
                Some((version, entry.path()))
            })
            .collect();
        versioned.sort_by_key(|(version, _)| std::cmp::Reverse(*version));

        versioned
            .into_iter()
            .map(|(_, path)| path)
            .chain(std::iter::once(system32.join("amdhip64.dll")))
            .collect()
    }

    /// `ROCm` installs `libamdhip64.so.<major>`; also try the unversioned name and the default
    /// `ROCm` prefix for setups that do not register the library with the dynamic linker.
    #[cfg(target_os = "linux")]
    fn library_paths() -> Vec<std::path::PathBuf> {
        [
            "libamdhip64.so.7",
            "libamdhip64.so.6",
            "libamdhip64.so.5",
            "libamdhip64.so",
            "/opt/rocm/lib/libamdhip64.so",
        ]
        .into_iter()
        .map(Into::into)
        .collect()
    }
}

#[cfg(test)]
mod test {
    use super::*;

    fn arch(major: u32, minor: u32, stepping: u32) -> AmdGpuArchInfo {
        AmdGpuArchInfo {
            major,
            minor,
            stepping,
        }
    }

    #[test]
    fn gfx_target_version_decodes_per_cep() {
        assert_eq!(
            AmdGpuArchInfo::from_gfx_target_version(90010),
            Some(arch(9, 0, 10))
        );
        assert_eq!(
            AmdGpuArchInfo::from_gfx_target_version(110501),
            Some(arch(11, 5, 1))
        );
        assert_eq!(
            AmdGpuArchInfo::from_gfx_target_version(120001),
            Some(arch(12, 0, 1))
        );
        // CPU nodes in the KFD topology report 0.
        assert_eq!(AmdGpuArchInfo::from_gfx_target_version(0), None);
    }

    #[test]
    fn target_id_decodes_per_cep() {
        assert_eq!(
            AmdGpuArchInfo::from_target_id("gfx90a"),
            Some(arch(9, 0, 10))
        );
        assert_eq!(
            AmdGpuArchInfo::from_target_id("gfx90a:xnack-"),
            Some(arch(9, 0, 10))
        );
        assert_eq!(
            AmdGpuArchInfo::from_target_id("gfx1100:xnack-:sramecc+"),
            Some(arch(11, 0, 0))
        );
        assert_eq!(
            AmdGpuArchInfo::from_target_id("gfx1151"),
            Some(arch(11, 5, 1))
        );
        assert_eq!(
            AmdGpuArchInfo::from_target_id("gfx1201"),
            Some(arch(12, 0, 1))
        );
        // Minor and stepping are hexadecimal, the major is decimal.
        assert_eq!(
            AmdGpuArchInfo::from_target_id("gfx2011a"),
            Some(arch(201, 1, 10))
        );

        assert_eq!(AmdGpuArchInfo::from_target_id(""), None);
        assert_eq!(AmdGpuArchInfo::from_target_id("gfx"), None);
        assert_eq!(AmdGpuArchInfo::from_target_id("gfx9"), None);
        assert_eq!(AmdGpuArchInfo::from_target_id("gfx9a"), None);
        assert_eq!(AmdGpuArchInfo::from_target_id("sm_86"), None);
        assert_eq!(AmdGpuArchInfo::from_target_id("gfxa0a"), None);
        assert_eq!(AmdGpuArchInfo::from_target_id("gfx90z"), None);
    }

    #[test]
    fn isa_version_round_trips_through_version_string() {
        let info = arch(9, 0, 10);
        assert_eq!(info.to_string(), "9.0.10");
        assert_eq!(info.to_version(), Version::from_str("9.0.10").unwrap());
        assert_eq!("9.0.10".parse::<AmdGpuArchInfo>(), Ok(info));
        assert_eq!("11.05.1".parse::<AmdGpuArchInfo>(), Ok(arch(11, 5, 1)));

        for invalid in [
            "", "9", "9.0", "9.0.10.0", "9.0.a", "gfx90a", "9..10", ".0.10", "9.0.10.",
        ] {
            assert!(invalid.parse::<AmdGpuArchInfo>().is_err(), "{invalid:?}");
        }
    }

    #[test]
    fn selection_prefers_compute_units_then_isa_version() {
        let device = |arch, compute_units| AmdGpuDevice {
            arch,
            compute_units,
            name: None,
        };

        // Framework Laptop 16: the newer integrated 780M (gfx1103, 12 CUs) loses to the discrete
        // RX 7700S (gfx1102, 32 CUs).
        let devices = [
            device(arch(11, 0, 3), Some(12)),
            device(arch(11, 0, 2), Some(32)),
        ];
        assert_eq!(select_device(&devices), Some(&devices[1]));

        // Equal compute units: the highest ISA version wins.
        let devices = [
            device(arch(11, 5, 1), Some(40)),
            device(arch(10, 1, 0), Some(40)),
        ];
        assert_eq!(select_device(&devices), Some(&devices[0]));

        // An unknown compute-unit count sorts below any known one.
        let devices = [
            device(arch(12, 0, 1), None),
            device(arch(9, 0, 10), Some(1)),
        ];
        assert_eq!(select_device(&devices), Some(&devices[1]));

        // Only unknown counts: fall back to the ISA version.
        let devices = [device(arch(9, 0, 10), None), device(arch(12, 0, 1), None)];
        assert_eq!(select_device(&devices), Some(&devices[1]));

        assert_eq!(select_device(&[]), None);
    }

    #[test]
    fn info_never_reports_arch_without_presence() {
        assert_eq!(
            info_from_devices(&[], false),
            AmdGpuInfo {
                present: false,
                arch_info: None
            }
        );
        assert_eq!(
            info_from_devices(&[], true),
            AmdGpuInfo {
                present: true,
                arch_info: None
            }
        );
        let device = AmdGpuDevice {
            arch: arch(11, 0, 0),
            compute_units: Some(96),
            name: None,
        };
        let expected_arch = device.arch;
        assert_eq!(
            info_from_devices(&[device], false),
            AmdGpuInfo {
                present: true,
                arch_info: Some(expected_arch)
            }
        );
    }

    /// Times a single cold `__amdgpu` + `__amdgpu_arch` detection, as the first AMD probe in this
    /// process.
    ///
    /// On Linux with `ROCm` this only reads sysfs; on Windows (and on Linux without KFD) it loads the
    /// HIP runtime, whose initialization may wake an idle GPU, so compare runs in the same driver
    /// state. Run on its own so nothing else has warmed up the driver:
    ///
    /// ```text
    /// cargo test -p rattler_virtual_packages --release -- --ignored --nocapture --exact \
    ///     amdgpu::test::bench_cold_detect
    /// ```
    #[test]
    #[ignore = "benchmark, run manually and in isolation"]
    fn bench_cold_detect() {
        let start = std::time::Instant::now();
        let info = detect_amdgpu_info();
        println!(
            "cold __amdgpu + __amdgpu_arch: {:?}  -> {info:?}",
            start.elapsed()
        );
    }

    #[test]
    fn detect_doesnt_crash() {
        let info = detect_amdgpu_info();
        println!("{info:#?}");
        assert!(info.present || info.arch_info.is_none());
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn kfd_properties_are_parsed() {
        // Abridged from a Radeon 8060S (gfx1151) node.
        let gpu = "\
cpu_cores_count 0
simd_count 80
mem_banks_count 1
caches_count 141
simd_per_cu 2
max_slots_scratch_cu 32
gfx_target_version 110501
vendor_id 4098
device_id 5536
";
        assert_eq!(
            linux::parse_kfd_properties(gpu),
            Some(AmdGpuDevice {
                arch: arch(11, 5, 1),
                compute_units: Some(40),
                name: None,
            })
        );

        // CPU nodes have no GPU target.
        let cpu = "cpu_cores_count 16\nsimd_count 0\nsimd_per_cu 0\ngfx_target_version 0\n";
        assert_eq!(linux::parse_kfd_properties(cpu), None);

        // A missing or zero divisor leaves the compute-unit count unknown.
        let no_cu = "gfx_target_version 90010\nsimd_count 416\nsimd_per_cu 0\n";
        assert_eq!(
            linux::parse_kfd_properties(no_cu),
            Some(AmdGpuDevice {
                arch: arch(9, 0, 10),
                compute_units: None,
                name: None,
            })
        );
        assert_eq!(linux::parse_kfd_properties(""), None);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn kfd_topology_and_drm_are_read_from_sysfs_layout() {
        let root = tempfile::tempdir().unwrap();

        // KFD topology: one CPU node and two GPU nodes.
        let nodes = root.path().join("nodes");
        for (name, properties) in [
            ("0", "gfx_target_version 0\nsimd_count 0\nsimd_per_cu 0\n"),
            (
                "1",
                "gfx_target_version 110003\nsimd_count 24\nsimd_per_cu 2\n",
            ),
            (
                "2",
                "gfx_target_version 110002\nsimd_count 64\nsimd_per_cu 2\n",
            ),
        ] {
            let node = nodes.join(name);
            std::fs::create_dir_all(&node).unwrap();
            std::fs::write(node.join("properties"), properties).unwrap();
        }
        let devices = linux::kfd_devices(&nodes);
        assert_eq!(devices.len(), 2);
        assert_eq!(
            select_device(&devices).map(|device| device.arch),
            Some(arch(11, 0, 2))
        );
        assert!(linux::kfd_devices(&root.path().join("missing")).is_empty());

        // DRM: an Intel card bound to i915 and an AMD card bound to amdgpu.
        let drm = root.path().join("drm");
        let drivers = root.path().join("drivers");
        for (card, vendor, driver) in [
            ("card0", "0x8086\n", "i915"),
            ("card1", "0x1002\n", "amdgpu"),
        ] {
            let device = drm.join(card).join("device");
            std::fs::create_dir_all(&device).unwrap();
            std::fs::write(device.join("vendor"), vendor).unwrap();
            let driver_dir = drivers.join(driver);
            std::fs::create_dir_all(&driver_dir).unwrap();
            std::os::unix::fs::symlink(&driver_dir, device.join("driver")).unwrap();
        }
        assert!(linux::drm_has_amdgpu(&drm));

        // An AMD card without the amdgpu driver (e.g. bound to radeon or vfio) does not count.
        std::fs::remove_file(drm.join("card1/device/driver")).unwrap();
        std::os::unix::fs::symlink(drivers.join("i915"), drm.join("card1/device/driver")).unwrap();
        assert!(!linux::drm_has_amdgpu(&drm));
        assert!(!linux::drm_has_amdgpu(&root.path().join("missing")));
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn amd_pci_device_ids_are_recognized() {
        assert!(windows::is_amd_pci_device_id(
            "PCI\\VEN_1002&DEV_73FF&SUBSYS_0E3D1002&REV_C1\\4&2D2E5D1F&0&0008"
        ));
        assert!(windows::is_amd_pci_device_id("pci\\ven_1002&dev_1900"));
        assert!(!windows::is_amd_pci_device_id("PCI\\VEN_10DE&DEV_2484"));
        assert!(!windows::is_amd_pci_device_id("PCI\\VEN_8086&DEV_9A49"));
        assert!(!windows::is_amd_pci_device_id(""));
    }
}
