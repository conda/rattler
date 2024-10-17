use std::path::Path;
use windows::{
    core::*,
    Win32::System::Com::*,
    Win32::System::Com::StructuredStorage::*,
    Win32::UI::Shell::*,
    Win32::UI::Shell::PropertiesSystem::*,
};

#[derive(thiserror::Error, Debug)]
enum CreateShortcutFail {
    #[error("Failed to create shortcut: {0}")]
    WindowsError(#[from] windows::core::Error),

    #[error("Failed to initialize COM")]
    CoInitializeFail,
}

/// Create a shortcut at the specified path.
pub(crate) fn create_shortcut(
    path: &Path,
    description: &str,
    filename: &Path,
    arguments: Option<&str>,
    workdir: Option<&Path>,
    iconpath: Option<&Path>,
    iconindex: i32,
    app_id: Option<&str>,
) -> std::result::Result<(), CreateShortcutFail> {
    unsafe {
        if !CoInitialize(None).is_ok() {
            return Err(CreateShortcutFail::CoInitializeFail);
        }

        let shell_link: IShellLinkW = CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER)?;
        let persist_file: IPersistFile = shell_link.cast()?;

        shell_link.SetPath(&HSTRING::from(path))?;
        shell_link.SetDescription(&HSTRING::from(description))?;

        if let Some(args) = arguments {
            shell_link.SetArguments(&HSTRING::from(args))?;
        }

        if let Some(icon) = iconpath {
            shell_link.SetIconLocation(&HSTRING::from(icon), iconindex)?;
        }

        if let Some(dir) = workdir {
            shell_link.SetWorkingDirectory(&HSTRING::from(dir))?;
        }

        if let Some(id) = app_id {
            let property_store: IPropertyStore = shell_link.cast()?;
            let mut prop_variant = PROPVARIANT::default();
            // PropVariantInit(&mut prop_variant);
            // SetPropStringValue(PCWSTR(PWSTR::from_raw(to_wide_string(id).as_mut_ptr())), &mut prop_variant)?;
            // property_store.SetValue(&PROPERTYKEY_AppUserModel_ID, &prop_variant)?;
            property_store.Commit()?;
            PropVariantClear(&mut prop_variant)?;
        }

        persist_file.Save(&HSTRING::from(filename), true)?;

        CoUninitialize();
        Ok(())
    }
}