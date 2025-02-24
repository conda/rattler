use crate::MenuMode;
use windows::Win32::UI::Shell::{SHChangeNotify, SHCNE_ASSOCCHANGED, SHCNF_IDLIST};

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

/// Registers a file extension handler in the Windows Registry.
///
/// Associates a file extension with a command and optional metadata (e.g., icon, app name) under
/// either the system-wide (`HKEY_LOCAL_MACHINE`) or current user (`HKEY_CURRENT_USER`) registry hive,
/// depending on the `mode`.
///
/// # Arguments
/// * `file_extension` - A `FileExtension` struct containing registration details.
/// * `mode` - The `MenuMode` specifying whether to register system-wide (`System`) or for the current user (`User`).
///
/// # Errors
/// Returns a `std::io::Error` if registry operations fail (e.g., insufficient permissions or invalid keys).
///
/// # Examples
/// ```ignore
/// let file_ext = FileExtension {
///     extension: ".test",
///     identifier: "TestApp.File",
///     command: "\"C:\\Test\\App.exe\" \"%1\"",
///     icon: Some("C:\\Test\\icon.ico"),
///     app_name: Some("Test App"),
///     app_user_model_id: None,
///     friendly_type_name: Some("Test File"),
/// };
/// register_file_extension(file_ext, MenuMode::User)?;
/// ```
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
        windows_registry::LOCAL_MACHINE
    } else {
        windows_registry::CURRENT_USER
    };

    let classes = hkey.create(r"Software\Classes")?;

    // Associate extension with handler
    let ext_key = classes.create(format!(r"{extension}\OpenWithProgids"))?;
    ext_key.set_string(identifier, "")?;

    // Register the handler
    let handler_desc = format!("{extension} {identifier} file");
    classes.create(identifier)?.set_string("", &handler_desc)?;

    // Set the 'open' command
    classes
        .create(format!(r"{identifier}\shell\open\command"))?
        .set_string("", command)?;

    // Set app name related values if provided
    if let Some(name) = app_name {
        let open_key = classes.create(format!(r"{identifier}\shell\open"))?;
        open_key.set_string("", name)?;
        open_key.set_string("FriendlyAppName", name)?;
        classes
            .create(identifier)?
            .set_string("FriendlyAppName", name)?;
    }

    // Set app user model ID if provided
    if let Some(id) = app_user_model_id {
        classes
            .create(identifier)?
            .set_string("AppUserModelID", id)?;
    }

    // Set icon if provided
    if let Some(icon_path) = icon {
        // Set default icon and shell open icon
        classes
            .create(identifier)?
            .set_string("DefaultIcon", icon_path)?;
        classes
            .create(format!(r"{identifier}\shell\open"))?
            .set_string("Icon", icon_path)?;
    }

    // Set friendly type name if provided
    // NOTE: Windows <10 requires the string in a PE file, but we just set the raw string
    if let Some(friendly_name) = friendly_type_name {
        classes
            .create(identifier)?
            .set_string("FriendlyTypeName", friendly_name)?;
    }

    Ok(())
}

/// Unregisters a file extension handler from the Windows Registry.
///
/// Removes the association between a file extension and its identifier, as well as the handler's registry entries,
/// based on the specified `mode` (system-wide or current user).
///
/// # Arguments
/// * `extension` - The file extension (e.g., `.test`) to unregister.
/// * `identifier` - The identifier of the handler (e.g., `TestApp.File`) to remove.
/// * `mode` - The `MenuMode` specifying the registry hive (`System` or `User`).
///
/// # Errors
/// Returns a `std::io::Error` if registry operations fail (e.g., insufficient permissions or key not found).
///
/// # Examples
/// ```ignore
/// unregister_file_extension(".test", "TestApp.File", MenuMode::User)?;
/// ```
pub fn unregister_file_extension(
    extension: &str,
    identifier: &str,
    mode: MenuMode,
) -> Result<(), std::io::Error> {
    let hkey = if mode == MenuMode::System {
        windows_registry::LOCAL_MACHINE
    } else {
        windows_registry::CURRENT_USER
    };

    let classes = hkey.create(r"Software\Classes")?;

    // Delete the identifier key
    classes.remove_tree(identifier)?;

    // Remove the association in OpenWithProgids
    let ext_key = classes.create(format!(r"{extension}\OpenWithProgids"));

    match ext_key {
        Ok(key) => {
            if key.get_string(identifier).is_err() {
                tracing::debug!(
                    "Handler '{}' is not associated with extension '{}'",
                    identifier,
                    extension
                );
            } else {
                key.remove_value(identifier)?;
            }
        }
        Err(e) => {
            tracing::error!("Could not check key '{}' for deletion: {}", extension, e);
            return Err(e.into());
        }
    }

    Ok(())
}

/// Converts a string to title case, capitalizing the first letter of each word.
///
/// Words are separated by whitespace, hyphens, or underscores. This is used internally
/// to format URL protocol names.
///
/// # Arguments
/// * `s` - The input string to convert.
///
/// # Returns
/// A new `String` in title case.
///
/// # Examples
/// ```ignore
/// assert_eq!(title_case("hello-world"), "Hello-World");
/// assert_eq!(title_case("my_url"), "My_Url");
/// ```
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

/// Represents a URL protocol registration with associated metadata.
///
/// This struct defines the properties needed to register a custom URL protocol handler in the Windows Registry,
/// such as a command to execute and optional application details.
#[derive(Debug, Clone, Copy)]
pub struct UrlProtocol<'a> {
    /// The URL protocol name (e.g., `rattlertest`).
    pub protocol: &'a str,
    /// The command to execute when the protocol is invoked.
    pub command: &'a str,
    /// The identifier for the protocol handler.
    pub identifier: &'a str,
    /// The path to an icon file to associate with the protocol.
    pub icon: Option<&'a str>,
    /// The friendly name of the application.
    pub app_name: Option<&'a str>,
    /// The application user model ID (AUMI) for the protocol handler.
    pub app_user_model_id: Option<&'a str>,
}

/// Registers a custom URL protocol handler in the Windows Registry.
///
/// Creates registry entries for a URL protocol (e.g., `myapp://`) with a command and optional metadata,
/// under either `HKEY_CLASSES_ROOT` (system-wide) or `HKEY_CURRENT_USER` (user-specific) based on `mode`.
///
/// # Arguments
/// * `url_protocol` - A `UrlProtocol` struct containing registration details.
/// * `mode` - The `MenuMode` specifying the registry hive (`System` or `User`).
///
/// # Errors
/// Returns a `std::io::Error` if registry operations fail (e.g., insufficient permissions).
///
/// # Examples
/// ```ignore
/// let url_proto = UrlProtocol {
///     protocol: "myapp",
///     command: "\"C:\\MyApp\\App.exe\" \"%1\"",
///     identifier: "MyApp",
///     icon: Some("C:\\MyApp\\icon.ico"),
///     app_name: Some("My App"),
///     app_user_model_id: None,
/// };
/// register_url_protocol(url_proto, MenuMode::User)?;
/// ```
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
        windows_registry::CLASSES_ROOT
    } else {
        windows_registry::CURRENT_USER
    };

    let base_path = if mode == MenuMode::System {
        protocol.to_string()
    } else {
        format!(r"Software\Classes\{protocol}")
    };

    let protocol_key = hkey.create(base_path)?;

    protocol_key.set_string("", format!("URL:{}", title_case(protocol)))?;
    protocol_key.set_string("URL Protocol", "")?;

    protocol_key
        .create(r"shell\open\command")?
        .set_string("", command)?;

    if let Some(name) = app_name {
        // let open_key = command_key.create(r"shell\open")?;
        let open_key = protocol_key.create(r"shell\open")?;
        open_key.set_string("", name)?;
        open_key.set_string("FriendlyAppName", name)?;
        protocol_key.set_string("FriendlyAppName", name)?;
    }

    if let Some(icon_path) = icon {
        protocol_key.set_string("DefaultIcon", icon_path)?;
        let open_key = protocol_key.create(r"shell\open")?;
        open_key.set_string("Icon", icon_path)?;
    }

    if let Some(aumi) = app_user_model_id {
        protocol_key.set_string("AppUserModelId", aumi)?;
    }

    protocol_key.set_string("_menuinst", identifier)?;

    Ok(())
}

/// Unregisters a URL protocol handler from the Windows Registry.
///
/// Removes the registry entries for a URL protocol if the identifier matches, based on the specified `mode`.
/// Does nothing if the protocol or identifier doesn’t exist or doesn’t match.
///
/// # Arguments
/// * `protocol` - The protocol name (e.g., `myapp`) to unregister.
/// * `identifier` - The identifier of the handler to verify before removal.
/// * `mode` - The `MenuMode` specifying the registry hive (`System` or `User`).
///
/// # Errors
/// Returns a `std::io::Error` if registry operations fail (e.g., insufficient permissions).
///
/// # Examples
/// ```ignore
/// unregister_url_protocol("myapp", "MyApp", MenuMode::User)?;
/// ```
pub fn unregister_url_protocol(
    protocol: &str,
    identifier: &str,
    mode: MenuMode,
) -> Result<(), std::io::Error> {
    let hkey = if mode == MenuMode::System {
        windows_registry::CLASSES_ROOT
    } else {
        windows_registry::CURRENT_USER
    };

    let base_path = if mode == MenuMode::System {
        protocol.to_string()
    } else {
        format!(r"Software\Classes\{protocol}")
    };

    if let Ok(key) = hkey.create(&base_path) {
        if let Ok(value) = key.get_string("_menuinst") {
            if value == identifier {
                hkey.remove_tree(&base_path)?;
            } else {
                return Ok(());
            }
        }
    }

    Ok(())
}

/// Notifies the Windows shell of changes to file associations or protocols.
///
/// Calls `SHChangeNotify` with `SHCNE_ASSOCCHANGED` to refresh the shell’s understanding of registered
/// file extensions or URL protocols. This is typically called after registration/unregistration.
///
/// # Safety
/// This function uses an unsafe Windows API call but is safe under normal conditions as it passes
/// valid parameters.
///
/// # Examples
/// ```ignore
/// register_file_extension(file_ext, MenuMode::User)?;
/// notify_shell_changes();
/// ```
pub fn notify_shell_changes() {
    unsafe {
        SHChangeNotify(SHCNE_ASSOCCHANGED, SHCNF_IDLIST, None, None);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn cleanup_registry(extension: &str, identifier: &str, mode: MenuMode) {
        let _ = unregister_file_extension(extension, identifier, mode);
    }

    fn cleanup_protocol(protocol: &str, identifier: &str, mode: MenuMode) {
        let _ = unregister_url_protocol(protocol, identifier, mode);
    }

    #[test]
    fn test_title_case() {
        let inputs = [
            ("hello-world", "Hello-World"),
            ("my_url", "My_Url"),
            ("my_url_protocol", "My_Url_Protocol"),
            ("my_url_protocol_test", "My_Url_Protocol_Test"),
            ("my_url_protocol_test_2", "My_Url_Protocol_Test_2"),
        ];

        for (input, expected) in inputs.iter() {
            assert_eq!(title_case(input), *expected);
        }
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
        let classes = windows_registry::CURRENT_USER.open(r"Software\Classes")?;

        let ext_key = classes.open(format!("{extension}\\OpenWithProgids"))?;
        assert!(ext_key.get_string(identifier).is_ok());

        let cmd_key = classes.open(format!("{identifier}\\shell\\open\\command"))?;
        let cmd_value: String = cmd_key.get_string("")?;
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
        let classes = windows_registry::CURRENT_USER.open(r"Software\Classes")?;
        let icon_key = classes.open(identifier)?;
        let icon_value = icon_key.get_string("DefaultIcon")?;
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
        let classes = windows_registry::CURRENT_USER.open(r"Software\Classes")?;

        assert!(classes.open(identifier).is_err());

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
        let key = windows_registry::CURRENT_USER.open(format!(r"Software\Classes\{protocol}"))?;

        let cmd_key = key.open(r"shell\open\command")?;
        let cmd_value = cmd_key.get_string("")?;
        assert_eq!(cmd_value, command);

        let id_value: String = key.get_string("_menuinst")?;
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
        let key = windows_registry::CURRENT_USER.open(r"Software\Classes")?;
        assert!(key.open(protocol).is_err());

        Ok(())
    }
}
