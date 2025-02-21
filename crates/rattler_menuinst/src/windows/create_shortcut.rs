use std::path::Path;

use windows::{
    core::{Interface, Result, GUID, HSTRING},
    Win32::Storage::EnhancedStorage::PKEY_AppUserModel_ID,
    Win32::System::Com::StructuredStorage::{InitPropVariantFromStringAsVector, PropVariantClear},
    Win32::System::Com::{
        CoCreateInstance, CoInitialize, CoUninitialize, IPersistFile, CLSCTX_INPROC_SERVER,
    },
    Win32::UI::Shell::{IShellLinkW, PropertiesSystem, ShellLink},
};
use PropertiesSystem::IPropertyStore;

#[derive(Debug, Clone, Copy)]
pub struct Shortcut<'a> {
    pub path: &'a str,
    pub description: &'a str,
    pub filename: &'a Path,
    pub arguments: Option<&'a str>,
    pub workdir: Option<&'a str>,
    pub iconpath: Option<&'a str>,
    pub iconindex: Option<i32>,
    pub app_id: Option<&'a str>,
}

/// Create a Windows `.lnk` shortcut file at the specified path.
pub fn create_shortcut(shortcut: Shortcut<'_>) -> Result<()> {
    let Shortcut {
        path,
        description,
        filename,
        arguments,
        workdir,
        iconpath,
        iconindex,
        app_id,
    } = shortcut;

    tracing::info!("Creating shortcut: {:?} at {}", filename, path);

    unsafe {
        // Initialize COM
        let co = CoInitialize(None);
        assert!(!co.is_err(), "Failed to initialize COM");

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

    fn create_test_shortcut(filename: &'static str) -> Shortcut<'static> {
        Shortcut {
            path: r"C:\Windows\notepad.exe",
            description: "Test Shortcut",
            filename: Path::new(filename),
            arguments: None,
            workdir: None,
            iconpath: None,
            iconindex: None,
            app_id: None,
        }
    }

    fn cleanup(filename: &str) {
        let path = Path::new(filename);
        if path.exists() {
            fs::remove_file(path).unwrap();
        }
    }

    #[test]
    fn test_basic_shortcut_creation() {
        let shortcut = create_test_shortcut("test_basic.lnk");
        let result = create_shortcut(shortcut);

        assert!(result.is_ok());
        assert!(shortcut.filename.exists());
        cleanup("test_basic.lnk");
    }

    #[test]
    fn test_shortcut_with_arguments() {
        let mut shortcut = create_test_shortcut("test_args.lnk");
        shortcut.arguments = Some("/A test.txt");
        let result = create_shortcut(shortcut);

        assert!(result.is_ok());
        assert!(shortcut.filename.exists());
        cleanup("test_args.lnk");
    }

    #[test]
    fn test_shortcut_with_all_options() {
        let shortcut = Shortcut {
            path: r"C:\Windows\notepad.exe",
            description: "Full Options Shortcut",
            filename: Path::new("test_full.lnk"),
            arguments: Some("/A"),
            workdir: Some(r"C:\Temp"),
            iconpath: Some(r"C:\Windows\notepad.exe"),
            iconindex: Some(0),
            app_id: Some("MyApp.TestShortcut"),
        };
        let result = create_shortcut(shortcut);

        assert!(result.is_ok());
        assert!(Path::new("test_full.lnk").exists());
        fs::remove_file("test_full.lnk").unwrap();
    }

    #[test]
    fn test_invalid_path() {
        let mut shortcut = create_test_shortcut("test_invalid.lnk");
        shortcut.path = r"C:\NonExistent\fake.exe";
        let result = create_shortcut(shortcut);

        assert!(result.is_ok()); // Note: Windows API doesn't validate path existence
        cleanup("test_invalid.lnk");
    }

    #[test]
    fn test_invalid_save_location() {
        let shortcut = Shortcut {
            path: r"C:\Windows\notepad.exe",
            description: "Invalid Save",
            filename: Path::new(r"C:\NonExistent\Directory\test.lnk"),
            arguments: None,
            workdir: None,
            iconpath: None,
            iconindex: None,
            app_id: None,
        };
        let result = create_shortcut(shortcut);

        assert!(result.is_err());
    }
}
