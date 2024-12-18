use std::path::Path;

use windows::{
    core::*, Win32::Storage::EnhancedStorage::PKEY_AppUserModel_ID,
    Win32::System::Com::StructuredStorage::*, Win32::System::Com::*, Win32::UI::Shell::*,
};
use PropertiesSystem::IPropertyStore;

/// Create a Windows `.lnk` shortcut file at the specified path.
pub fn create_shortcut(
    path: &str,
    description: &str,
    filename: &Path,
    arguments: Option<&str>,
    workdir: Option<&str>,
    iconpath: Option<&str>,
    iconindex: Option<i32>,
    app_id: Option<&str>,
) -> Result<()> {
    tracing::info!("Creating shortcut: {:?} at {}", filename, path);

    unsafe {
        // Initialize COM
        let co = CoInitialize(None);
        if co.is_err() {
            panic!("Failed to initialize COM");
        }

        let shell_link: IShellLinkW =
            CoCreateInstance(&ShellLink as *const GUID, None, CLSCTX_INPROC_SERVER)?;

        // Get IPersistFile interface
        let persist_file: IPersistFile = shell_link.cast()?;

        // Set required properties
        shell_link.SetPath(&HSTRING::from(path))?;
        shell_link.SetDescription(&HSTRING::from(description))?;

        // Set optional properties
        if let Some(args) = arguments {
            shell_link.SetArguments(&HSTRING::from(args))?;
        }

        if let Some(work_dir) = workdir {
            shell_link.SetWorkingDirectory(&HSTRING::from(work_dir))?;
        }

        if let Some(icon_path) = iconpath {
            shell_link.SetIconLocation(&HSTRING::from(icon_path), iconindex.unwrap_or(0))?;
        }

        // Handle App User Model ID if provided
        if let Some(app_id_str) = app_id {
            let property_store: IPropertyStore = shell_link.cast()?;
            let mut prop_variant = InitPropVariantFromStringAsVector(&HSTRING::from(app_id_str))?;
            property_store.SetValue(&PKEY_AppUserModel_ID, &prop_variant)?;
            property_store.Commit()?;
            PropVariantClear(&mut prop_variant)?;
        }

        // Save the shortcut
        persist_file.Save(&HSTRING::from(filename), true)?;

        CoUninitialize();
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::Path;

    #[test]
    fn test_basic_shortcut_creation() {
        let result = create_shortcut(
            "C:\\Windows\\notepad.exe",
            "Notepad Shortcut",
            "test_basic.lnk",
            None,
            None,
            None,
            None,
            None,
        );

        assert!(result.is_ok());
        assert!(Path::new("test_basic.lnk").exists());
        fs::remove_file("test_basic.lnk").unwrap();
    }

    #[test]
    fn test_shortcut_with_arguments() {
        let result = create_shortcut(
            "C:\\Windows\\notepad.exe",
            "Notepad with Args",
            "test_args.lnk",
            Some("/A test.txt"),
            None,
            None,
            None,
            None,
        );

        assert!(result.is_ok());
        assert!(Path::new("test_args.lnk").exists());
        fs::remove_file("test_args.lnk").unwrap();
    }

    #[test]
    fn test_shortcut_with_all_options() {
        let result = create_shortcut(
            "C:\\Windows\\notepad.exe",
            "Full Options Shortcut",
            "test_full.lnk",
            Some("/A"),
            Some("C:\\Temp"),
            Some("C:\\Windows\\notepad.exe"),
            Some(0),
            Some("MyApp.TestShortcut"),
        );

        assert!(result.is_ok());
        assert!(Path::new("test_full.lnk").exists());
        fs::remove_file("test_full.lnk").unwrap();
    }

    #[test]
    fn test_invalid_path() {
        let result = create_shortcut(
            "C:\\NonExistent\\fake.exe",
            "Invalid Path",
            "test_invalid.lnk",
            None,
            None,
            None,
            None,
            None,
        );

        assert!(result.is_ok()); // Note: Windows API doesn't validate path existence
        if Path::new("test_invalid.lnk").exists() {
            fs::remove_file("test_invalid.lnk").unwrap();
        }
    }

    #[test]
    fn test_invalid_save_location() {
        let result = create_shortcut(
            "C:\\Windows\\notepad.exe",
            "Invalid Save",
            "C:\\NonExistent\\Directory\\test.lnk",
            None,
            None,
            None,
            None,
            None,
        );

        assert!(result.is_err());
    }
}
