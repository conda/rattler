//! Low-level functions to detect the Windows version on the system. See
//! [`windows_version`].

use once_cell::sync::OnceCell;
use rattler_conda_types::Version;

/// Returns the Windows version of the current platform.
///
/// Returns an error if determining the Windows version resulted in an error.
/// Returns `None` if the Windows version could not be determined. Note that
/// this does not mean the current platform is not Windows.
pub fn windows_version() -> Option<Version> {
    static DETECTED_WINDOWS_VERSION: OnceCell<Option<Version>> = OnceCell::new();
    DETECTED_WINDOWS_VERSION
        .get_or_init(detect_windows_version)
        .clone()
}

#[cfg(target_os = "windows")]
fn detect_windows_version() -> Option<Version> {
    let windows_version = winver::WindowsVersion::detect()?;
    Some(
        std::str::FromStr::from_str(&windows_version.to_string())
            .expect("WindowsVersion::to_string() should always return a valid version"),
    )
}

#[cfg(not(target_os = "windows"))]
const fn detect_windows_version() -> Option<Version> {
    None
}

/// Returns the Plug and Play device instance ids matching `filter`, or `None` if they cannot be
/// enumerated.
///
/// `filter` and `flags` are passed straight to `CM_Get_Device_ID_ListW`: with
/// `CM_GETIDLIST_FILTER_ENUMERATOR` the filter is an enumerator such as `PCI`, with
/// `CM_GETIDLIST_FILTER_CLASS` it is a device setup class GUID. Only configuration manager metadata
/// is read, so this never initializes a driver or wakes a powered-down device.
#[cfg(target_os = "windows")]
pub(crate) fn device_instance_ids(
    filter: &str,
    flags: u32,
) -> Option<impl Iterator<Item = String>> {
    use windows_sys::Win32::Devices::DeviceAndDriverInstallation::{
        CM_Get_Device_ID_List_SizeW, CM_Get_Device_ID_ListW, CR_SUCCESS,
    };

    let filter: Vec<u16> = filter.encode_utf16().chain(std::iter::once(0)).collect();

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
    let mut start = 0;
    Some(std::iter::from_fn(move || {
        let rest = &buffer[start..];
        let len = rest.iter().position(|&c| c == 0).unwrap_or(rest.len());
        if len == 0 {
            return None;
        }
        start = (start + len + 1).min(buffer.len());
        Some(String::from_utf16_lossy(&rest[..len]))
    }))
}

#[cfg(test)]
mod test {
    #[test]
    pub fn doesnt_crash() {
        let version = super::detect_windows_version();
        println!("Windows {version:?}");
    }
}
