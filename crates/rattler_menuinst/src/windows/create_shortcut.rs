use windows::{
    core::*,
    Win32::System::Com::*,
    Win32::UI::Shell::*,
    Win32::System::Com::StructuredStorage::*,
    Win32::Storage::EnhancedStorage::PKEY_AppUserModel_ID,
};
use PropertiesSystem::IPropertyStore;

fn create_shortcut(
    path: String,
    description: String,
    filename: String,
    arguments: Option<String>,
    workdir: Option<String>,
    iconpath: Option<String>,
    iconindex: Option<i32>,
    app_id: Option<String>,
) -> Result<()> {
    unsafe {
        // Initialize COM
        let co = CoInitialize(None);
        if co.is_err() {
            panic!("Failed to initialize COM");
        }

        let shell_link: IShellLinkW = CoCreateInstance(
            &ShellLink as *const GUID,
            None,
            CLSCTX_INPROC_SERVER
        )?;

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
            shell_link.SetIconLocation(
                &HSTRING::from(icon_path),
                iconindex.unwrap_or(0)
            )?;
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
            String::from("C:\\Windows\\notepad.exe"),
            String::from("Notepad Shortcut"),
            String::from("test_basic.lnk"),
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
            String::from("C:\\Windows\\notepad.exe"),
            String::from("Notepad with Args"),
            String::from("test_args.lnk"),
            Some(String::from("/A test.txt")),
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
            String::from("C:\\Windows\\notepad.exe"),
            String::from("Full Options Shortcut"),
            String::from("test_full.lnk"),
            Some(String::from("/A")),
            Some(String::from("C:\\Temp")),
            Some(String::from("C:\\Windows\\notepad.exe")),
            Some(0),
            Some(String::from("MyApp.TestShortcut")),
        );

        assert!(result.is_ok());
        assert!(Path::new("test_full.lnk").exists());
        fs::remove_file("test_full.lnk").unwrap();
    }

    #[test]
    fn test_invalid_path() {
        let result = create_shortcut(
            String::from("C:\\NonExistent\\fake.exe"),
            String::from("Invalid Path"),
            String::from("test_invalid.lnk"),
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
            String::from("C:\\Windows\\notepad.exe"),
            String::from("Invalid Save"),
            String::from("C:\\NonExistent\\Directory\\test.lnk"),
            None,
            None,
            None,
            None,
            None,
        );

        assert!(result.is_err());
    }
}
