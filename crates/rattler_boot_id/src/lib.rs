//! Identification of the machine's current boot session.
//!
//! Caches whose entries must not outlive a reboot, such as detected hardware or the results of a
//! virtual package detector, store the [`BootId`] observed when the entry was written and compare
//! it with [`BootId::current`] when reading.

use serde::{Deserialize, Serialize};
#[cfg(target_os = "windows")]
use windows_sys::Win32::System::Registry::{HKEY_LOCAL_MACHINE, RRF_RT_REG_DWORD, RegGetValueW};

/// Identifies a single boot session of the machine.
///
/// All variants are compiled on every platform so the comparison logic can be unit-tested
/// anywhere; `current` only ever produces the variants that exist on the host platform.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BootId {
    /// The kernel's per-boot UUID from `/proc/sys/kernel/random/boot_id` (Linux).
    Uuid(String),
    /// The prefetcher boot counter from the registry, incremented once per boot (Windows).
    BootCount(u32),
    /// A boot time in unix seconds: read from `kern.boottime` (macOS) or derived from the uptime
    /// (Windows fallback). The derivation drifts a little between processes, which `matches`
    /// absorbs with a tolerance.
    BootTime(u64),
}

impl BootId {
    /// Returns the identifier of the current boot session, or `None` if it cannot be determined
    /// (in which case no caching takes place).
    pub fn current() -> Option<Self> {
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
        #[cfg(target_os = "macos")]
        {
            // The kernel records the boot time in `kern.boottime`; it is stable for the whole
            // session, so no tolerance is needed beyond the one shared with the Windows fallback.
            Some(Self::BootTime(macos_boot_time()?))
        }
        #[cfg(not(any(target_os = "linux", target_os = "windows", target_os = "macos")))]
        {
            None
        }
    }

    /// Returns true if both identifiers refer to the same boot session.
    pub fn matches(&self, other: &Self) -> bool {
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
fn boot_times_within_tolerance(a: u64, b: u64) -> bool {
    /// The derived boot time drifts a little between processes.
    const BOOT_TIME_TOLERANCE_SECS: u64 = 120;
    a.abs_diff(b) <= BOOT_TIME_TOLERANCE_SECS
}

/// Reads the prefetcher boot counter from the registry, incremented once per boot.
#[cfg(target_os = "windows")]
fn windows_boot_count() -> Option<u32> {
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

/// Reads the boot time in unix seconds from the `kern.boottime` sysctl.
#[cfg(target_os = "macos")]
fn macos_boot_time() -> Option<u64> {
    let mut mib = [libc::CTL_KERN, libc::KERN_BOOTTIME];
    let mut boottime = libc::timeval {
        tv_sec: 0,
        tv_usec: 0,
    };
    let mut size = std::mem::size_of::<libc::timeval>();
    // SAFETY: `mib` names a kernel variable of type `timeval`, `boottime` is a properly sized
    // and aligned buffer for it, and `size` carries its length in and out.
    let status = unsafe {
        libc::sysctl(
            mib.as_mut_ptr(),
            mib.len() as libc::c_uint,
            std::ptr::addr_of_mut!(boottime).cast::<libc::c_void>(),
            &mut size,
            std::ptr::null_mut(),
            0,
        )
    };
    if status != 0 || size != std::mem::size_of::<libc::timeval>() {
        return None;
    }
    u64::try_from(boottime.tv_sec).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn same_kind_must_match_exactly_except_boot_times() {
        assert!(BootId::Uuid("a".into()).matches(&BootId::Uuid("a".into())));
        assert!(!BootId::Uuid("a".into()).matches(&BootId::Uuid("b".into())));
        assert!(BootId::BootCount(3).matches(&BootId::BootCount(3)));
        assert!(!BootId::BootCount(3).matches(&BootId::BootCount(4)));
        assert!(BootId::BootTime(1000).matches(&BootId::BootTime(1100)));
        assert!(!BootId::BootTime(1000).matches(&BootId::BootTime(2000)));
    }

    #[test]
    fn boot_time_tolerance_boundary() {
        assert!(boot_times_within_tolerance(1_000, 1_000));
        assert!(boot_times_within_tolerance(1_000, 1_120));
        assert!(boot_times_within_tolerance(1_120, 1_000));
        assert!(!boot_times_within_tolerance(1_000, 1_121));
    }

    #[test]
    fn different_kinds_never_match() {
        assert!(!BootId::Uuid("3".into()).matches(&BootId::BootCount(3)));
        assert!(!BootId::BootCount(1000).matches(&BootId::BootTime(1000)));
    }

    #[cfg(any(target_os = "linux", target_os = "windows", target_os = "macos"))]
    #[test]
    fn current_boot_session_is_known_and_stable() {
        let first = BootId::current().expect("the boot session is known on this platform");
        let second = BootId::current().unwrap();
        assert!(first.matches(&second));
    }

    #[test]
    fn serialization_is_tagged_by_kind() {
        let json = serde_json::to_string(&BootId::BootTime(42)).unwrap();
        assert_eq!(json, r#"{"boot_time":42}"#);
        assert_eq!(
            serde_json::from_str::<BootId>(r#"{"uuid":"abc"}"#).unwrap(),
            BootId::Uuid("abc".into())
        );
    }
}
