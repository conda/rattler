use anyhow::{Context, Result};
use winreg::enums::*;
use winreg::RegKey;

pub fn register_file_extension(
    extension: &str,
    identifier: &str,
    command: &str,
    icon: Option<&str>,
    mode: &str,
) -> Result<()> {
    let hkey = if mode == "system" {
        HKEY_LOCAL_MACHINE
    } else {
        HKEY_CURRENT_USER
    };

    let classes = RegKey::predef(hkey).open_subkey_with_flags("Software\\Classes", KEY_ALL_ACCESS)?;

    // Associate extension with handler
    let ext_key = classes.create_subkey(&format!("{}\\OpenWithProgids", extension))?;
    ext_key.0.set_value(identifier, &"")?;
    tracing::debug!("Created registry entry for extension '{}'", extension);

    // Register the handler
    let handler_desc = format!("{} {} handler", extension, identifier);
    classes.create_subkey(identifier)?.0.set_value("", &handler_desc)?;
    tracing::debug!("Created registry entry for handler '{}'", identifier);

    // Set the 'open' command
    let command_key = classes.create_subkey(&format!("{}\\shell\\open\\command", identifier))?;
    command_key.0.set_value("", &command)?;
    debug!("Created registry entry for command '{}'", command);

    // Set icon if provided
    if let Some(icon_path) = icon {
        let icon_key = classes.create_subkey(identifier)?;
        icon_key.0.set_value("DefaultIcon", &icon_path)?;
        tracing::debug!("Created registry entry for icon '{}'", icon_path);
    }

    Ok(())
}

pub fn unregister_file_extension(extension: &str, identifier: &str, mode: &str) -> Result<()> {
    let hkey = if mode == "system" {
        HKEY_LOCAL_MACHINE
    } else {
        HKEY_CURRENT_USER
    };

    let classes = RegKey::predef(hkey).open_subkey_with_flags("Software\\Classes", KEY_ALL_ACCESS)?;

    // Delete the identifier key
    classes.delete_subkey_all(identifier)?;

    // Remove the association in OpenWithProgids
    let ext_key = classes.open_subkey_with_flags(
        &format!("{}\\OpenWithProgids", extension),
        KEY_ALL_ACCESS,
    );

    match ext_key {
        Ok(key) => {
            if key.get_value::<String, _>(identifier).is_err() {
                debug!(
                    "Handler '{}' is not associated with extension '{}'",
                    identifier, extension
                );
            } else {
                key.delete_value(identifier)?;
            }
        }
        Err(e) => {
            tracing::error!(
                "Could not check key '{}' for deletion: {}",
                extension,
                e
            );
            return Err(e.into());
        }
    }

    Ok(())
}

pub fn register_url_protocol(
    protocol: &str,
    command: &str,
    identifier: Option<&str>,
    icon: Option<&str>,
    mode: &str,
) -> Result<()> {
    let key = if mode == "system" {
        RegKey::predef(HKEY_CLASSES_ROOT).create_subkey(protocol)?
    } else {
        RegKey::predef(HKEY_CURRENT_USER).create_subkey(&format!("Software\\Classes\\{}", protocol))?
    };

    key.0.set_value("", &format!("URL:{}", protocol.to_uppercase()))?;
    key.0.set_value("URL Protocol", &"")?;

    let command_key = key.0.create_subkey(r"shell\open\command")?;
    command_key.set_value("", &command)?;

    if let Some(icon_path) = icon {
        key.0.set_value("DefaultIcon", &icon_path)?;
    }

    if let Some(id) = identifier {
        key.0.set_value("_menuinst", &id)?;
    }

    Ok(())
}

pub fn unregister_url_protocol(protocol: &str, identifier: Option<&str>, mode: &str) -> Result<()> {
    let key = if mode == "system" {
        RegKey::predef(HKEY_CLASSES_ROOT)
    } else {
        RegKey::predef(HKEY_CURRENT_USER).open_subkey("Software\\Classes")?
    };

    let delete = match key.open_subkey(protocol) {
        Ok(k) => {
            match k.get_value::<String, _>("_menuinst") {
                Ok(value) => identifier.is_none() || Some(value.as_str()) == identifier,
                Err(_) => identifier.is_none(),
            }
        }
        Err(e) => {
            tracing::error!("Could not check key {} for deletion: {}", protocol, e);
            return Ok(());
        }
    };

    if delete {
        key.delete_subkey_all(protocol)?;
    }

    Ok(())
}
