use windows::Win32::UI::Shell::{SHChangeNotify, SHCNE_ASSOCCHANGED, SHCNF_IDLIST};
use winreg::enums::{HKEY_CLASSES_ROOT, HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, KEY_ALL_ACCESS};
use winreg::RegKey;

use crate::MenuMode;

#[derive(Debug, Clone, Copy)]
pub struct FileExtension<'a> {
    pub extension: &'a str,
    pub identifier: &'a str,
    pub command: &'a str,
    pub icon: Option<&'a str>,
    pub app_name: Option<&'a str>,
    pub app_user_model_id: Option<&'a str>,
    pub friendly_type_name: Option<&'a str>,
}

pub fn register_file_extension(
    file_extension: FileExtension<'_>,
    mode: MenuMode,
) -> Result<(), std::io::Error> {
    let FileExtension {
        extension,
        identifier,
        command,
        icon,
        app_name,
        app_user_model_id,
        friendly_type_name,
    } = file_extension;

    let hkey = if mode == MenuMode::System {
        HKEY_LOCAL_MACHINE
    } else {
        HKEY_CURRENT_USER
    };

    let classes =
        RegKey::predef(hkey).open_subkey_with_flags("Software\\Classes", KEY_ALL_ACCESS)?;

    // Associate extension with handler
    let ext_key = classes.create_subkey(format!("{extension}\\OpenWithProgids"))?;
    ext_key.0.set_value(identifier, &"")?;

    // Register the handler
    let handler_desc = format!("{extension} {identifier} file");
    classes
        .create_subkey(identifier)?
        .0
        .set_value("", &handler_desc)?;

    // Set the 'open' command
    let command_key = classes.create_subkey(format!("{identifier}\\shell\\open\\command"))?;
    command_key.0.set_value("", &command)?;

    // Set app name related values if provided
    if let Some(name) = app_name {
        let open_key = classes.create_subkey(format!("{identifier}\\shell\\open"))?;
        open_key.0.set_value("", &name)?;
        classes
            .create_subkey(identifier)?
            .0
            .set_value("FriendlyAppName", &name)?;
        classes
            .create_subkey(format!("{identifier}\\shell\\open"))?
            .0
            .set_value("FriendlyAppName", &name)?;
    }

    // Set app user model ID if provided
    if let Some(id) = app_user_model_id {
        classes
            .create_subkey(identifier)?
            .0
            .set_value("AppUserModelID", &id)?;
    }

    // Set icon if provided
    if let Some(icon_path) = icon {
        // Set default icon and shell open icon
        classes
            .create_subkey(identifier)?
            .0
            .set_value("DefaultIcon", &icon_path)?;
        classes
            .create_subkey(format!("{identifier}\\shell\\open"))?
            .0
            .set_value("Icon", &icon_path)?;
    }

    // Set friendly type name if provided
    // NOTE: Windows <10 requires the string in a PE file, but we just set the raw string
    if let Some(friendly_name) = friendly_type_name {
        classes
            .create_subkey(identifier)?
            .0
            .set_value("FriendlyTypeName", &friendly_name)?;
    }

    Ok(())
}

pub fn unregister_file_extension(
    extension: &str,
    identifier: &str,
    mode: MenuMode,
) -> Result<(), std::io::Error> {
    let hkey = if mode == MenuMode::System {
        HKEY_LOCAL_MACHINE
    } else {
        HKEY_CURRENT_USER
    };

    let classes =
        RegKey::predef(hkey).open_subkey_with_flags("Software\\Classes", KEY_ALL_ACCESS)?;

    // Delete the identifier key
    classes.delete_subkey_all(identifier)?;

    // Remove the association in OpenWithProgids
    let ext_key =
        classes.open_subkey_with_flags(format!("{extension}\\OpenWithProgids"), KEY_ALL_ACCESS);

    match ext_key {
        Ok(key) => {
            if key.get_value::<String, _>(identifier).is_err() {
                tracing::debug!(
                    "Handler '{}' is not associated with extension '{}'",
                    identifier,
                    extension
                );
            } else {
                key.delete_value(identifier)?;
            }
        }
        Err(e) => {
            tracing::error!("Could not check key '{}' for deletion: {}", extension, e);
            return Err(e);
        }
    }

    Ok(())
}

fn title_case(s: &str) -> String {
    let mut result = String::with_capacity(s.len());
    let mut capitalize_next = true;

    for c in s.chars() {
        if c.is_whitespace() || c == '-' || c == '_' {
            capitalize_next = true;
            result.push(c);
        } else if capitalize_next {
            result.extend(c.to_uppercase());
            capitalize_next = false;
        } else {
            result.extend(c.to_lowercase());
        }
    }

    result
}

#[derive(Debug, Clone, Copy)]
pub struct UrlProtocol<'a> {
    pub protocol: &'a str,
    pub command: &'a str,
    pub identifier: &'a str,
    pub icon: Option<&'a str>,
    pub app_name: Option<&'a str>,
    pub app_user_model_id: Option<&'a str>,
}

pub fn register_url_protocol(
    url_protocol: UrlProtocol<'_>,
    mode: MenuMode,
) -> Result<(), std::io::Error> {
    let UrlProtocol {
        protocol,
        command,
        identifier,
        icon,
        app_name,
        app_user_model_id,
    } = url_protocol;

    let hkey = if mode == MenuMode::System {
        HKEY_CLASSES_ROOT
    } else {
        HKEY_CURRENT_USER
    };

    let base_path = if mode == MenuMode::System {
        protocol.to_string()
    } else {
        format!("Software\\Classes\\{protocol}")
    };

    let classes = RegKey::predef(hkey);
    let protocol_key = classes.create_subkey(&base_path)?;

    protocol_key
        .0
        .set_value("", &format!("URL:{}", title_case(protocol)))?;
    protocol_key.0.set_value("URL Protocol", &"")?;

    let command_key = protocol_key.0.create_subkey("shell\\open\\command")?;
    command_key.0.set_value("", &command)?;

    if let Some(name) = app_name {
        let open_key = protocol_key.0.create_subkey("shell\\open")?;
        open_key.0.set_value("", &name)?;
        protocol_key.0.set_value("FriendlyAppName", &name)?;
        open_key.0.set_value("FriendlyAppName", &name)?;
    }

    if let Some(icon_path) = icon {
        protocol_key.0.set_value("DefaultIcon", &icon_path)?;
        let open_key = protocol_key.0.create_subkey("shell\\open")?;
        open_key.0.set_value("Icon", &icon_path)?;
    }

    if let Some(aumi) = app_user_model_id {
        protocol_key.0.set_value("AppUserModelId", &aumi)?;
    }

    protocol_key.0.set_value("_menuinst", &identifier)?;

    Ok(())
}

pub fn unregister_url_protocol(
    protocol: &str,
    identifier: &str,
    mode: MenuMode,
) -> Result<(), std::io::Error> {
    let hkey = if mode == MenuMode::System {
        HKEY_CLASSES_ROOT
    } else {
        HKEY_CURRENT_USER
    };

    let base_path = if mode == MenuMode::System {
        protocol.to_string()
    } else {
        format!("Software\\Classes\\{protocol}")
    };

    if let Ok(key) = RegKey::predef(hkey).open_subkey(&base_path) {
        if let Ok(value) = key.get_value::<String, _>("_menuinst") {
            if value != identifier {
                return Ok(());
            }
        }
        let _ = RegKey::predef(hkey).delete_subkey_all(&base_path);
    }

    Ok(())
}

pub fn notify_shell_changes() {
    unsafe {
        SHChangeNotify(SHCNE_ASSOCCHANGED, SHCNF_IDLIST, None, None);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use winreg::RegKey;

    fn cleanup_registry(extension: &str, identifier: &str, mode: MenuMode) {
        let _ = unregister_file_extension(extension, identifier, mode);
    }

    fn cleanup_protocol(protocol: &str, identifier: &str, mode: MenuMode) {
        let _ = unregister_url_protocol(protocol, identifier, mode);
    }

    #[test]
    fn test_register_file_extension_user() -> std::io::Result<()> {
        let extension = ".rattlertest";
        let identifier = "TestApp.File";
        let command = "\"C:\\Test\\App.exe\" \"%1\"";
        let mode = MenuMode::User;

        // Cleanup before test
        cleanup_registry(extension, identifier, mode);

        let file_extension = FileExtension {
            extension,
            identifier,
            command,
            icon: None,
            app_name: None,
            app_user_model_id: None,
            friendly_type_name: None,
        };
        // Test registration
        register_file_extension(file_extension, mode)?;

        // Verify registration
        let classes = RegKey::predef(HKEY_CURRENT_USER).open_subkey("Software\\Classes")?;

        let ext_key = classes.open_subkey(format!("{extension}\\OpenWithProgids"))?;
        assert!(ext_key.get_value::<String, _>(identifier).is_ok());

        let cmd_key = classes.open_subkey(format!("{identifier}\\shell\\open\\command"))?;
        let cmd_value: String = cmd_key.get_value("")?;
        assert_eq!(cmd_value, command);

        // Cleanup
        cleanup_registry(extension, identifier, mode);
        Ok(())
    }

    #[test]
    fn test_register_file_extension_with_icon() -> std::io::Result<()> {
        let extension = ".yrattlertest";
        let identifier = "yTestApp.File";
        let command = "\"C:\\Test\\App.exe\" \"%1\"";
        let icon = "C:\\Test\\icon.ico";
        let mode = MenuMode::User;
        let app_name = Some("Test App");
        let app_user_model_id = Some("TestApp");
        let friendly_type_name = Some("Test App File");

        let file_extension = FileExtension {
            extension,
            identifier,
            command,
            icon: Some(icon),
            app_name,
            app_user_model_id,
            friendly_type_name,
        };

        // Test registration with icon
        register_file_extension(file_extension, mode)?;

        // Verify icon
        let classes = RegKey::predef(HKEY_CURRENT_USER).open_subkey("Software\\Classes")?;
        let icon_key = classes.open_subkey(identifier)?;
        let icon_value: String = icon_key.get_value("DefaultIcon")?;
        assert_eq!(icon_value, icon);

        // Cleanup
        cleanup_registry(extension, identifier, mode);
        Ok(())
    }

    #[test]
    fn test_unregister_file_extension() -> std::io::Result<()> {
        let extension = ".xrattlertest";
        let identifier = "xTestApp.File";
        let command = "\"C:\\Test\\App.exe\" \"%1\"";
        let mode = MenuMode::User;

        let file_extension = FileExtension {
            extension,
            identifier,
            command,
            icon: None,
            app_name: None,
            app_user_model_id: None,
            friendly_type_name: None,
        };

        // Setup
        register_file_extension(file_extension, mode)?;

        // Test unregistration
        unregister_file_extension(extension, identifier, mode)?;

        // Verify removal
        let classes = RegKey::predef(HKEY_CURRENT_USER).open_subkey("Software\\Classes")?;

        assert!(classes.open_subkey(identifier).is_err());

        Ok(())
    }

    #[test]
    fn test_register_url_protocol() -> std::io::Result<()> {
        let protocol = "rattlertest";
        let command = "\"C:\\Test\\App.exe\" \"%1\"";
        let identifier = "TestApp";
        let app_name = Some("Test App");
        let icon = Some("C:\\Test\\icon.ico");
        let app_user_model_id = Some("TestApp");
        let mode = MenuMode::User;

        // Cleanup before test
        cleanup_protocol(protocol, identifier, mode);

        let url_protocol = UrlProtocol {
            protocol,
            command,
            identifier,
            icon,
            app_name,
            app_user_model_id,
        };

        // Test registration
        register_url_protocol(url_protocol, mode)?;

        // Verify registration
        let key = RegKey::predef(HKEY_CURRENT_USER)
            .open_subkey(format!("Software\\Classes\\{protocol}"))?;

        let cmd_key = key.open_subkey(r"shell\open\command")?;
        let cmd_value: String = cmd_key.get_value("")?;
        assert_eq!(cmd_value, command);

        let id_value: String = key.get_value("_menuinst")?;
        assert_eq!(id_value, identifier);

        // Cleanup
        cleanup_protocol(protocol, identifier, mode);
        Ok(())
    }

    #[test]
    fn test_unregister_url_protocol() -> std::io::Result<()> {
        let protocol = "rattlertest-2";
        let command = "\"C:\\Test\\App.exe\" \"%1\"";
        let identifier = "xTestApp";
        let app_name = Some("Test App");
        let icon = Some("C:\\Test\\icon.ico");
        let app_user_model_id = Some("TestApp");
        let mode = MenuMode::User;

        let url_protocol = UrlProtocol {
            protocol,
            command,
            identifier,
            icon,
            app_name,
            app_user_model_id,
        };

        // Setup
        register_url_protocol(url_protocol, mode)?;

        // Test unregistration
        unregister_url_protocol(protocol, identifier, mode)?;

        // Verify removal
        let key = RegKey::predef(HKEY_CURRENT_USER).open_subkey("Software\\Classes")?;
        assert!(key.open_subkey(protocol).is_err());

        Ok(())
    }
}
