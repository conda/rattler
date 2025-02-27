use std::{
    io::Write,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    process::Command,
};

use fs_err as fs;
use fs_err::File;
use plist::Value;
use rattler_conda_types::{menuinst::MacOsTracker, Platform};
use rattler_shell::{
    activation::{ActivationError, ActivationVariables, Activator},
    shell,
};
use sha2::{Digest as _, Sha256};

use crate::utils::slugify;
use crate::{
    render::{BaseMenuItemPlaceholders, MenuItemPlaceholders, PlaceholderString},
    schema::{
        CFBundleDocumentTypesModel, CFBundleTypeRole, CFBundleURLTypesModel, LSHandlerRank, MacOS,
        MacOSVersion, MenuItemCommand, UTTypeDeclarationModel,
    },
    utils::{log_output, run_pre_create_command},
    MenuInstError, MenuMode,
};
use std::collections::HashMap;

pub fn quote_args<I, S>(args: I) -> Vec<String>
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    args.into_iter()
        .map(|arg| format!(r#""{}""#, arg.as_ref()))
        .collect()
}

#[derive(Debug, Clone)]
pub struct MacOSMenu {
    name: String,
    prefix: PathBuf,
    item: MacOS,
    command: MenuItemCommand,
    directories: Directories,
    placeholders: MenuItemPlaceholders,
}

#[derive(Debug, Clone)]
pub struct Directories {
    /// Path to the .app directory defining the menu item
    location: PathBuf,
    /// Path to the nested .app directory defining the menu item main
    /// application
    nested_location: PathBuf,
}

impl Directories {
    pub fn new(menu_mode: MenuMode, bundle_name: &str) -> Self {
        let base_location = match menu_mode {
            MenuMode::System => PathBuf::from("/"),
            MenuMode::User => dirs::home_dir().expect("Failed to get home directory"),
        };

        let location = base_location.join("Applications").join(bundle_name);
        let nested_location = location.join("Contents/Resources").join(bundle_name);

        Self {
            location,
            nested_location,
        }
    }

    fn resources(&self) -> PathBuf {
        self.location.join("Contents/Resources")
    }

    fn nested_resources(&self) -> PathBuf {
        self.nested_location.join("Contents/Resources")
    }

    pub fn create_directories(
        &self,
        needs_appkit_launcher: bool,
        tracker: &mut MacOsTracker,
    ) -> Result<(), MenuInstError> {
        tracker.app_folder = self.location.clone();

        fs::create_dir_all(self.location.join("Contents/Resources"))?;
        fs::create_dir_all(self.location.join("Contents/MacOS"))?;

        if needs_appkit_launcher {
            fs::create_dir_all(self.nested_location.join("Contents/Resources"))?;
            fs::create_dir_all(self.nested_location.join("Contents/MacOS"))?;
        }

        Ok(())
    }
}

impl UTTypeDeclarationModel {
    fn to_plist(&self, placeholders: &MenuItemPlaceholders) -> Value {
        let mut type_dict = plist::Dictionary::new();
        type_dict.insert(
            "UTTypeConformsTo".into(),
            Value::Array(
                self.ut_type_conforms_to
                    .iter()
                    .map(|s| Value::String(s.resolve(placeholders)))
                    .collect(),
            ),
        );
        if let Some(desc) = &self.ut_type_description {
            type_dict.insert(
                "UTTypeDescription".into(),
                Value::String(desc.resolve(placeholders)),
            );
        }
        if let Some(icon) = &self.ut_type_icon_file {
            type_dict.insert(
                "UTTypeIconFile".into(),
                Value::String(icon.resolve(placeholders)),
            );
        }
        type_dict.insert(
            "UTTypeIdentifier".into(),
            Value::String(self.ut_type_identifier.resolve(placeholders)),
        );
        if let Some(url) = &self.ut_type_reference_url {
            type_dict.insert(
                "UTTypeReferenceURL".into(),
                Value::String(url.resolve(placeholders)),
            );
        }

        let mut tag_spec = plist::Dictionary::new();
        for (k, v) in &self.ut_type_tag_specification {
            tag_spec.insert(
                k.resolve(placeholders),
                Value::Array(
                    v.iter()
                        .map(|s| Value::String(s.resolve(placeholders)))
                        .collect(),
                ),
            );
        }

        type_dict.insert("UTTypeTagSpecification".into(), Value::Dictionary(tag_spec));
        Value::Dictionary(type_dict)
    }
}

impl CFBundleDocumentTypesModel {
    fn to_plist(&self, placeholders: &MenuItemPlaceholders) -> Value {
        let mut type_dict = plist::Dictionary::new();
        type_dict.insert(
            "CFBundleTypeName".into(),
            Value::String(self.cf_bundle_type_name.resolve(placeholders)),
        );

        if let Some(icon) = &self.cf_bundle_type_icon_file {
            type_dict.insert(
                "CFBundleTypeIconFile".into(),
                Value::String(icon.resolve(placeholders)),
            );
        }

        if let Some(role) = &self.cf_bundle_type_role {
            type_dict.insert("CFBundleTypeRole".into(), role.to_plist());
        }

        type_dict.insert(
            "LSItemContentTypes".into(),
            Value::Array(
                self.ls_item_content_types
                    .iter()
                    .map(|s| s.resolve(placeholders).into())
                    .collect(),
            ),
        );

        type_dict.insert("LSHandlerRank".into(), self.ls_handler_rank.to_plist());

        Value::Dictionary(type_dict)
    }
}

impl CFBundleURLTypesModel {
    fn to_plist(&self, placeholders: &MenuItemPlaceholders) -> Value {
        let mut type_dict = plist::Dictionary::new();

        if let Some(role) = self.cf_bundle_type_role.clone() {
            type_dict.insert("CFBundleTypeRole".into(), role.to_plist());
        }

        type_dict.insert(
            "CFBundleURLSchemes".into(),
            Value::Array(
                self.cf_bundle_url_schemes
                    .iter()
                    .map(|s| s.resolve(placeholders).into())
                    .collect(),
            ),
        );

        type_dict.insert(
            "CFBundleURLName".into(),
            Value::String(self.cf_bundle_url_name.resolve(placeholders)),
        );

        if let Some(icon) = &self.cf_bundle_url_icon_file {
            type_dict.insert(
                "CFBundleURLIconFile".into(),
                Value::String(icon.resolve(placeholders)),
            );
        }

        Value::Dictionary(type_dict)
    }
}

impl MacOSVersion {
    pub fn to_plist(&self) -> plist::Value {
        plist::Value::String(
            self.0
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<String>>()
                .join("."),
        )
    }
}

impl LSHandlerRank {
    pub fn to_plist(&self) -> plist::Value {
        plist::Value::String(
            match self {
                LSHandlerRank::Owner => "Owner",
                LSHandlerRank::Default => "Default",
                LSHandlerRank::Alternate => "Alternate",
                LSHandlerRank::None => "None",
            }
            .to_string(),
        )
    }
}

impl CFBundleTypeRole {
    pub fn to_plist(&self) -> plist::Value {
        plist::Value::String(
            match self {
                CFBundleTypeRole::Editor => "Editor",
                CFBundleTypeRole::Viewer => "Viewer",
                CFBundleTypeRole::Shell => "Shell",
                CFBundleTypeRole::QLGenerator => "QLGenerator",
                CFBundleTypeRole::None => "None",
            }
            .to_string(),
        )
    }
}

/// Call `lsregister` with args
fn lsregister(args: &[&str], directory: &Path) -> Result<(), MenuInstError> {
    let exe = "/System/Library/Frameworks/CoreServices.framework/Frameworks/LaunchServices.framework/Support/lsregister";
    tracing::debug!("Calling lsregister with args: {:?}", args);
    let output = Command::new(exe)
        .args(args)
        .arg(directory)
        .output()
        .map_err(|e| MenuInstError::InstallError(format!("failed to execute lsregister: {e}")))?;

    if !output.status.success() {
        log_output("lsregister", output);
    }

    Ok(())
}

impl MacOSMenu {
    fn new_impl(
        prefix: &Path,
        item: MacOS,
        command: MenuItemCommand,
        menu_mode: MenuMode,
        placeholders: &BaseMenuItemPlaceholders,
        directories: Option<Directories>,
    ) -> Self {
        let name = command
            .name
            .resolve(crate::schema::Environment::Base, placeholders);

        let bundle_name = format!("{name}.app");
        let directories = directories.unwrap_or(Directories::new(menu_mode, &bundle_name));
        tracing::info!("Editing menu item for {bundle_name}");

        let refined_placeholders = placeholders.refine(&directories.location);
        Self {
            name,
            prefix: prefix.to_path_buf(),
            item,
            command,
            directories,
            placeholders: refined_placeholders,
        }
    }

    #[cfg(test)]
    pub fn new_with_directories(
        prefix: &Path,
        item: MacOS,
        command: MenuItemCommand,
        menu_mode: MenuMode,
        placeholders: &BaseMenuItemPlaceholders,
        directories: Directories,
    ) -> Self {
        Self::new_impl(
            prefix,
            item,
            command,
            menu_mode,
            placeholders,
            Some(directories),
        )
    }

    pub fn new(
        prefix: &Path,
        item: MacOS,
        command: MenuItemCommand,
        menu_mode: MenuMode,
        placeholders: &BaseMenuItemPlaceholders,
    ) -> Self {
        Self::new_impl(prefix, item, command, menu_mode, placeholders, None)
    }

    /// In macOS, file type and URL protocol associations are handled by the
    /// Apple Events system. When the user opens on a file or URL, the system
    /// will send an Apple Event to the application that was registered as a
    /// handler. We need a special launcher to handle these events and pass
    /// them to the wrapped application in the shortcut.
    ///
    /// See:
    /// - <https://developer.apple.com/library/archive/documentation/Carbon/Conceptual/LaunchServicesConcepts/LSCConcepts/LSCConcepts.html>
    /// - The source code at /src/appkit-launcher in this repository
    fn needs_appkit_launcher(&self) -> bool {
        self.item.event_handler.is_some()
    }

    // Run pre-create command
    pub fn precreate(&self) -> Result<(), MenuInstError> {
        if let Some(precreate) = &self.command.precreate {
            let pre_create_command = precreate.resolve(&self.placeholders);
            run_pre_create_command(&pre_create_command)?;
        }

        let Some(link_in_bundle) = &self.item.link_in_bundle else {
            return Ok(());
        };

        for (src, dest) in link_in_bundle {
            let src = src.resolve(&self.placeholders);
            let dest = dest.resolve(&self.placeholders);
            let dest = self.directories.location.join(&dest);
            if !dest.starts_with(&self.directories.location) {
                return Err(MenuInstError::InstallError(format!(
                    "'link_in_bundle' destinations MUST be created inside the .app bundle ({}), but it points to '{}'.",
                    self.directories.location.display(),
                    dest.display()
                )));
            }

            if let Some(parent) = dest.parent() {
                fs::create_dir_all(parent)?;
            }

            let src = fs::canonicalize(src)?;

            fs_err::os::unix::fs::symlink(&src, &dest)?;

            tracing::info!("Symlinked  {src:?} to {dest:?}",);
        }

        Ok(())
    }

    pub fn install_icon(&self) -> Result<(), MenuInstError> {
        if let Some(icon) = self.command.icon.as_ref() {
            let icon = PathBuf::from(icon.resolve(&self.placeholders));
            let icon_name = icon.file_name().expect("Failed to get icon name");
            let dest = self.directories.resources().join(icon_name);
            fs::copy(&icon, &dest)?;

            tracing::info!("Installed icon to {}", dest.display());

            if self.needs_appkit_launcher() {
                let dest = self.directories.nested_resources().join(icon_name);
                fs::copy(&icon, dest)?;
            }
        } else {
            tracing::info!("No icon to install");
        }

        Ok(())
    }

    fn write_pkg_info(&self) -> Result<(), MenuInstError> {
        let create_pkg_info = |path: &PathBuf, short_name: &str| -> Result<(), MenuInstError> {
            let path = path.join("Contents/PkgInfo");
            tracing::debug!("Writing pkg info to {}", path.display());
            let mut f = fs::File::create(&path)?;
            f.write_all(format!("APPL{short_name}").as_bytes())?;
            Ok(())
        };
        let short_name = slugify(&self.name.chars().take(8).collect::<String>());

        create_pkg_info(&self.directories.location, &short_name)?;
        if self.needs_appkit_launcher() {
            create_pkg_info(&self.directories.nested_location, &short_name)?;
        }

        Ok(())
    }

    fn write_plist_info(&self) -> Result<(), MenuInstError> {
        let name = self.name.clone();
        let slugname = slugify(&name);
        let shortname = if slugname.len() > 16 {
            let hashed = format!("{:x}", Sha256::digest(slugname.as_bytes()));
            format!("{}{}", &slugname[..10], &hashed[..6])
        } else {
            slugname.clone()
        };

        let mut pl = plist::Dictionary::new();

        let bundle_name = resolve(&self.item.cf_bundle_name, &self.placeholders, &shortname);
        pl.insert("CFBundleName".into(), Value::String(bundle_name));

        let display_name = resolve(&self.item.cf_bundle_display_name, &self.placeholders, &name);
        pl.insert("CFBundleDisplayName".into(), Value::String(display_name));

        // This one is _not_ part of the schema, so we just set it
        pl.insert("CFBundleExecutable".into(), Value::String(slugname.clone()));

        pl.insert(
            "CFBundleIdentifier".into(),
            Value::String(format!("com.{slugname}")),
        );
        pl.insert("CFBundlePackageType".into(), Value::String("APPL".into()));

        let cf_bundle_version = resolve(&self.item.cf_bundle_version, &self.placeholders, "1.0.0");
        pl.insert(
            "CFBundleVersion".into(),
            Value::String(cf_bundle_version.clone()),
        );

        pl.insert(
            "CFBundleGetInfoString".into(),
            Value::String(format!("{slugname}-{cf_bundle_version}")),
        );

        pl.insert(
            "CFBundleShortVersionString".into(),
            Value::String(cf_bundle_version),
        );

        if let Some(icon) = &self.command.icon {
            let resolved_icon = icon.resolve(&self.placeholders);
            if let Some(icon_name) = Path::new(&resolved_icon)
                .file_name()
                .and_then(|name| name.to_str())
            {
                pl.insert("CFBundleIconFile".into(), Value::String(icon_name.into()));
            } else {
                tracing::warn!("Failed to extract icon name from path: {:?}", resolved_icon);
            }
        }

        if let Some(cf_bundle_types_model) = &self.item.cf_bundle_document_types {
            let mut types_array = Vec::new();
            for cf_bundle_type in cf_bundle_types_model {
                types_array.push(cf_bundle_type.to_plist(&self.placeholders));
            }
            pl.insert("CFBundleDocumentTypes".into(), Value::Array(types_array));
        }

        if let Some(cf_bundle_spoken_names) = &self.item.cf_bundle_spoken_name {
            pl.insert(
                "CFBundleSpokenName".into(),
                Value::String(cf_bundle_spoken_names.resolve(&self.placeholders)),
            );
        }

        if self.needs_appkit_launcher() {
            tracing::debug!(
                "Writing plist to {}",
                self.directories
                    .nested_location
                    .join("Contents/Info.plist")
                    .display()
            );
            plist::to_file_xml(
                self.directories.nested_location.join("Contents/Info.plist"),
                &pl,
            )?;
            pl.insert("LSBackgroundOnly".into(), Value::Boolean(true));
            pl.insert(
                "CFBundleIdentifier".into(),
                Value::String(format!("com.{slugname}-appkit-launcher")),
            );
        }

        if let Some(category) = self.item.ls_application_category_type.as_ref() {
            pl.insert(
                "LSApplicationCategoryType".into(),
                Value::String(category.clone()),
            );
        }

        if let Some(background_only) = self.item.ls_background_only {
            pl.insert("LSBackgroundOnly".into(), Value::Boolean(background_only));
        }

        if let Some(env) = self.item.ls_environment.as_ref() {
            let mut env_dict = plist::Dictionary::new();
            for (k, v) in env {
                env_dict.insert(k.into(), Value::String(v.resolve(&self.placeholders)));
            }
            pl.insert("LSEnvironment".into(), Value::Dictionary(env_dict));
        }

        if let Some(version) = self.item.ls_minimum_system_version.as_ref() {
            pl.insert("LSMinimumSystemVersion".into(), version.to_plist());
        }

        if let Some(prohibited) = self.item.ls_multiple_instances_prohibited {
            pl.insert(
                "LSMultipleInstancesProhibited".into(),
                Value::Boolean(prohibited),
            );
        }

        if let Some(ns_supports_automatic_graphics_switching) =
            self.item.ns_supports_automatic_graphics_switching
        {
            pl.insert(
                "NSSupportsAutomaticGraphicsSwitching".into(),
                Value::Boolean(ns_supports_automatic_graphics_switching),
            );
        }

        if let Some(requires_native) = self.item.ls_requires_native_execution {
            pl.insert(
                "LSRequiresNativeExecution".into(),
                Value::Boolean(requires_native),
            );
        }

        if let Some(ut_exported_type_declarations) = &self.item.ut_exported_type_declarations {
            let mut type_array = Vec::new();
            for ut_type in ut_exported_type_declarations {
                type_array.push(ut_type.to_plist(&self.placeholders));
            }
            pl.insert(
                "UTExportedTypeDeclarations".into(),
                Value::Array(type_array),
            );
        }

        if let Some(ut_imported_type_declarations) = &self.item.ut_imported_type_declarations {
            let mut type_array = Vec::new();
            for ut_type in ut_imported_type_declarations {
                type_array.push(ut_type.to_plist(&self.placeholders));
            }
            pl.insert(
                "UTImportedTypeDeclarations".into(),
                Value::Array(type_array),
            );
        }

        if let Some(cf_bundle_url_types) = &self.item.cf_bundle_url_types {
            let mut url_array = Vec::new();
            for url_type in cf_bundle_url_types {
                url_array.push(url_type.to_plist(&self.placeholders));
            }
            pl.insert("CFBundleURLTypes".into(), Value::Array(url_array));
        }

        let plist_target = self.directories.location.join("Contents/Info.plist");
        tracing::info!("Writing plist to {:?}", plist_target);
        Ok(plist::to_file_xml(plist_target, &pl)?)
    }

    fn sign_with_entitlements(&self) -> Result<(), MenuInstError> {
        // write a plist file with the entitlements to the filesystem
        let Some(item_entitlements) = &self.item.entitlements else {
            return Ok(());
        };

        let mut entitlements = plist::Dictionary::new();

        for e in item_entitlements {
            entitlements.insert(e.clone(), Value::Boolean(true));
        }

        let entitlements_file = self
            .directories
            .location
            .join("Contents/Entitlements.plist");
        plist::to_file_xml(&entitlements_file, &entitlements)?;

        // sign the .app directory with the entitlements
        let _codesign = Command::new("/usr/bin/codesign")
            .arg("--verbose")
            .arg("--sign")
            .arg("-")
            .arg("--force")
            .arg("--deep")
            .arg("--options")
            .arg("runtime")
            .arg("--prefix")
            .arg(format!("com.{}", slugify(&self.name)))
            .arg("--entitlements")
            .arg(&entitlements_file)
            .arg(&self.directories.location)
            .output()?;

        Ok(())
    }

    fn command(&self) -> Result<String, ActivationError> {
        let mut lines = vec!["#!/bin/sh".to_string()];

        if self.command.terminal.unwrap_or(false) {
            lines.extend_from_slice(&[
                r#"if [ "${__CFBundleIdentifier:-}" != "com.apple.Terminal" ]; then"#.to_string(),
                r#"    open -b com.apple.terminal "$0""#.to_string(),
                r#"    exit $?"#.to_string(),
                "fi".to_string(),
            ]);
        }

        if let Some(working_dir) = self.command.working_dir.as_ref() {
            let working_dir = working_dir.resolve(&self.placeholders);
            fs::create_dir_all(&working_dir).expect("Failed to create working directory");
            lines.push(format!("cd \"{working_dir}\""));
        }

        if let Some(precommand) = &self.command.precommand {
            lines.push(precommand.resolve(&self.placeholders));
        }

        // Run a cached activation
        if self.command.activate.unwrap_or(false) {
            // create a bash activation script and emit it into the script
            let activator = Activator::from_path(&self.prefix, shell::Bash, Platform::current())?;
            let activation_env = activator.run_activation(ActivationVariables::default(), None)?;

            for (k, v) in activation_env {
                lines.push(format!(r#"export {k}="{v}""#));
            }
        }

        let command = self
            .command
            .command
            .iter()
            .map(|s| s.resolve(&self.placeholders));
        lines.push(quote_args(command).join(" "));

        Ok(lines.join("\n"))
    }

    fn write_appkit_launcher(&self) -> Result<PathBuf, MenuInstError> {
        #[cfg(target_arch = "aarch64")]
        let launcher_bytes = include_bytes!("../data/appkit_launcher_arm64");
        #[cfg(target_arch = "x86_64")]
        let launcher_bytes = include_bytes!("../data/appkit_launcher_x86_64");

        let launcher_path = self.default_appkit_launcher_path();
        fs::write(&launcher_path, launcher_bytes)?;
        fs::set_permissions(&launcher_path, std::fs::Permissions::from_mode(0o755))?;

        Ok(launcher_path)
    }

    fn write_launcher(&self) -> Result<PathBuf, MenuInstError> {
        #[cfg(target_arch = "aarch64")]
        let launcher_bytes = include_bytes!("../data/osx_launcher_arm64");
        #[cfg(target_arch = "x86_64")]
        let launcher_bytes = include_bytes!("../data/osx_launcher_x86_64");

        let launcher_path = self.default_launcher_path();
        let mut file = File::create(&launcher_path)?;
        file.write_all(launcher_bytes)?;
        fs::set_permissions(&launcher_path, std::fs::Permissions::from_mode(0o755))?;

        Ok(launcher_path)
    }

    fn write_script(&self, script_path: Option<PathBuf>) -> Result<PathBuf, MenuInstError> {
        let script_path = script_path.unwrap_or_else(|| {
            PathBuf::from(format!(
                "{}-script",
                self.default_launcher_path().to_string_lossy()
            ))
        });
        tracing::info!("Writing script to {}", script_path.display());
        let mut file = File::create(&script_path)?;
        file.write_all(self.command()?.as_bytes())?;
        fs::set_permissions(&script_path, std::fs::Permissions::from_mode(0o755))?;
        Ok(script_path)
    }

    fn write_event_handler(
        &self,
        script_path: Option<PathBuf>,
    ) -> Result<Option<PathBuf>, MenuInstError> {
        if !self.needs_appkit_launcher() {
            return Ok(None);
        }

        let event_handler_logic = match self.item.event_handler.as_ref() {
            Some(logic) => logic.resolve(&self.placeholders),
            None => return Ok(None),
        };

        let script_path = script_path.unwrap_or_else(|| {
            self.directories
                .location
                .join("Contents/Resources/handle-event")
        });

        let mut file = File::create(&script_path)?;
        file.write_all(format!("#!/bin/bash\n{event_handler_logic}\n").as_bytes())?;
        fs::set_permissions(&script_path, std::fs::Permissions::from_mode(0o755))?;
        Ok(Some(script_path))
    }

    fn default_appkit_launcher_path(&self) -> PathBuf {
        let name = slugify(&self.name);
        self.directories.location.join("Contents/MacOS").join(&name)
    }

    fn default_launcher_path(&self) -> PathBuf {
        let name = slugify(&self.name);
        if self.needs_appkit_launcher() {
            self.directories
                .nested_location
                .join("Contents/MacOS")
                .join(&name)
        } else {
            self.directories.location.join("Contents/MacOS").join(&name)
        }
    }

    fn register_launchservice(&self, tracker: &mut MacOsTracker) -> Result<(), MenuInstError> {
        if !self.needs_appkit_launcher() {
            return Ok(());
        }

        lsregister(&["-R"], &self.directories.location)?;
        tracker.lsregister = Some(self.directories.location.clone());
        Ok(())
    }

    pub fn install(&self, tracker: &mut MacOsTracker) -> Result<(), MenuInstError> {
        self.directories
            .create_directories(self.needs_appkit_launcher(), tracker)?;
        self.precreate()?;
        self.install_icon()?;
        self.write_pkg_info()?;
        self.write_plist_info()?;
        self.write_appkit_launcher()?;
        self.write_launcher()?;
        self.write_script(None)?;
        self.write_event_handler(None)?;
        self.register_launchservice(tracker)?;
        self.sign_with_entitlements()?;
        Ok(())
    }
}

pub(crate) fn install_menu_item(
    prefix: &Path,
    macos_item: MacOS,
    command: MenuItemCommand,
    placeholders: &BaseMenuItemPlaceholders,
    menu_mode: MenuMode,
) -> Result<MacOsTracker, MenuInstError> {
    let menu = MacOSMenu::new(prefix, macos_item, command, menu_mode, placeholders);
    let mut tracker = MacOsTracker::default();
    menu.install(&mut tracker)?;
    Ok(tracker)
}

pub(crate) fn remove_menu_item(tracker: &MacOsTracker) -> Result<Vec<PathBuf>, MenuInstError> {
    let mut removed = Vec::new();
    tracing::info!("Removing macOS menu item");
    match fs_err::remove_dir_all(&tracker.app_folder) {
        Ok(_) => {
            tracing::info!("Removed app folder: {}", tracker.app_folder.display());
            removed.push(tracker.app_folder.clone());
        }
        Err(e) => {
            tracing::warn!("Failed to remove app folder: {}", e);
        }
    }

    if let Some(lsregister_path) = &tracker.lsregister {
        match lsregister(&["-R", "-u", "-all"], lsregister_path) {
            Ok(_) => {
                tracing::info!(
                    "Unregistered with lsregister: {}",
                    lsregister_path.display()
                );
            }
            Err(e) => {
                tracing::warn!("Failed to unregister with lsregister: {}", e);
            }
        }
    }
    Ok(removed)
}

fn resolve(
    input: &Option<PlaceholderString>,
    placeholders: impl AsRef<HashMap<String, String>>,
    default: &str,
) -> String {
    match input {
        Some(s) => s.resolve(placeholders),
        None => default.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use crate::{schema::MenuInstSchema, test::test_data, MenuMode};
    use rattler_conda_types::menuinst::MacOsTracker;
    use std::{
        collections::HashMap,
        fs,
        path::{Path, PathBuf},
    };
    use tempfile::TempDir;

    impl super::Directories {
        pub fn new_test() -> Self {
            // Create a temporary directory for testing
            Self {
                location: tempfile::tempdir().unwrap().into_path(),
                nested_location: tempfile::tempdir().unwrap().into_path(),
            }
        }
    }

    #[test]
    fn test_directories() {
        let dirs = super::Directories::new_test();
        assert!(dirs.location.exists());
        assert!(dirs.nested_location.exists());
    }

    struct FakePlaceholders {
        placeholders: HashMap<String, String>,
    }

    impl AsRef<HashMap<String, String>> for FakePlaceholders {
        fn as_ref(&self) -> &HashMap<String, String> {
            &self.placeholders
        }
    }

    struct FakePrefix {
        _tmp_dir: TempDir,
        prefix_path: PathBuf,
        schema: MenuInstSchema,
    }

    impl FakePrefix {
        fn new(schema_json: &Path) -> Self {
            let tmp_dir = TempDir::new().unwrap();
            let prefix_path = tmp_dir.path().join("test-env");
            let schema_json = test_data().join(schema_json);
            let menu_folder = prefix_path.join("Menu");

            fs::create_dir_all(&menu_folder).unwrap();
            fs::copy(
                &schema_json,
                menu_folder.join(schema_json.file_name().unwrap()),
            )
            .unwrap();

            // Create a icon file for the
            let schema = std::fs::read_to_string(schema_json).unwrap();
            let parsed_schema: MenuInstSchema = serde_json::from_str(&schema).unwrap();

            let mut placeholders = HashMap::<String, String>::new();
            placeholders.insert(
                "MENU_DIR".to_string(),
                menu_folder.to_string_lossy().to_string(),
            );

            for item in &parsed_schema.menu_items {
                let icon = item.command.icon.as_ref().unwrap();
                for ext in &["icns", "png", "svg"] {
                    placeholders.insert("ICON_EXT".to_string(), (*ext).to_string());
                    let icon_path = icon.resolve(FakePlaceholders {
                        placeholders: placeholders.clone(),
                    });
                    fs::write(&icon_path, []).unwrap();
                }
            }

            fs::create_dir_all(prefix_path.join("bin")).unwrap();
            fs::write(prefix_path.join("bin/python"), []).unwrap();

            Self {
                _tmp_dir: tmp_dir,
                prefix_path,
                schema: parsed_schema,
            }
        }

        pub fn prefix(&self) -> &Path {
            &self.prefix_path
        }
    }

    #[test]
    fn test_macos_menu_installation() {
        let dirs = super::Directories::new_test();
        let fake_prefix = FakePrefix::new(Path::new("spyder/menu.json"));

        let placeholders = super::BaseMenuItemPlaceholders::new(
            fake_prefix.prefix(),
            fake_prefix.prefix(),
            rattler_conda_types::Platform::current(),
        );

        let item = fake_prefix.schema.menu_items[0].clone();
        let macos = item.platforms.osx.unwrap();
        let command = item.command.merge(macos.base);

        let menu = super::MacOSMenu::new_with_directories(
            fake_prefix.prefix(),
            macos.specific,
            command,
            MenuMode::User,
            &placeholders,
            dirs.clone(),
        );
        let mut tracker = MacOsTracker::default();
        menu.install(&mut tracker).unwrap();

        assert!(dirs.location.exists());
        assert!(dirs.nested_location.exists());

        // check that the plist file was created
        insta::assert_snapshot!(
            fs::read_to_string(dirs.location.join("Contents/Info.plist")).unwrap()
        );
    }

    /// Test macOS version parsing
    #[test]
    fn test_macos_version() {
        let version = super::MacOSVersion(vec![10, 15, 0]);
        assert_eq!(
            version.to_plist(),
            plist::Value::String("10.15.0".to_string())
        );

        // parsing from string
        let version: super::MacOSVersion = serde_json::from_str("\"10.15.0\"").unwrap();
        assert_eq!(version.0, vec![10, 15, 0]);
    }
}
