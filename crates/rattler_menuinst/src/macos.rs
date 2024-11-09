use crate::{schema::MacOS, slugify, utils, MenuInstError, MenuMode};
use fs_err as fs;
use fs_err::File;
use plist::Value;
use std::{
    io::{BufWriter, Write},
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    process::Command,
};

pub struct MacOSMenu {
    name: String,
    prefix: PathBuf,
    item: MacOS,
    directories: Directories,
}

pub struct Directories {
    /// Path to the .app directory defining the menu item
    location: PathBuf,
    /// Path to the nested .app directory defining the menu item main application
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

    pub fn create_directories(&self, needs_appkit_launcher: bool) -> Result<(), MenuInstError> {
        fs::create_dir_all(self.location.join("Contents/Resources"))?;
        fs::create_dir_all(self.location.join("Contents/MacOS"))?;

        if needs_appkit_launcher {
            fs::create_dir_all(self.nested_location.join("Contents/Resources"))?;
            fs::create_dir_all(self.nested_location.join("Contents/MacOS"))?;
        }

        Ok(())
    }
}

impl MacOSMenu {
    pub fn new(prefix: &Path, item: MacOS, directories: Directories) -> Self {
        Self {
            name: item
                .base
                .get_name(crate::schema::Environment::Base)
                .to_string(),
            prefix: prefix.to_path_buf(),
            item,
            directories,
        }
    }

    /// In macOS, file type and URL protocol associations are handled by the
    /// Apple Events system. When the user opens on a file or URL, the system
    /// will send an Apple Event to the application that was registered as a handler.
    /// We need a special launcher to handle these events and pass them to the
    /// wrapped application in the shortcut.
    ///
    /// See:
    /// - <https://developer.apple.com/library/archive/documentation/Carbon/Conceptual/LaunchServicesConcepts/LSCConcepts/LSCConcepts.html>
    /// - The source code at /src/appkit-launcher in this repository
    ///
    fn needs_appkit_launcher(&self) -> bool {
        self.item.cf_bundle_identifier.is_some() || self.item.cf_bundle_document_types.is_some()
    }

    pub fn install_icon(&self) -> Result<(), MenuInstError> {
        if let Some(icon) = self.item.base.icon.as_ref() {
            let icon = PathBuf::from(icon);
            let icon_name = icon.file_name().expect("Failed to get icon name");
            let dest = self.directories.resources().join(icon_name);
            fs::copy(&icon, &dest)?;

            println!("Installed icon to {}", dest.display());

            if self.needs_appkit_launcher() {
                let dest = self.directories.nested_resources().join(icon_name);
                fs::copy(&icon, dest)?;
            }
        } else {
            println!("No icon to install");
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
            // let hashed = format!("{:x}", Sha256::digest(slugname.as_bytes()));
            let hashed = "123456";
            format!("{}{}", &slugname[..10], &hashed[..6])
        } else {
            slugname.clone()
        };

        let mut pl = plist::Dictionary::new();
        pl.insert("CFBundleName".into(), Value::String(shortname));
        pl.insert("CFBundleDisplayName".into(), Value::String(name));
        pl.insert("CFBundleExecutable".into(), Value::String(slugname.clone()));
        pl.insert(
            "CFBundleGetInfoString".into(),
            Value::String(format!("{slugname}-1.0.0")),
        );
        pl.insert(
            "CFBundleIdentifier".into(),
            Value::String(format!("com.{slugname}")),
        );
        pl.insert("CFBundlePackageType".into(), Value::String("APPL".into()));
        pl.insert("CFBundleVersion".into(), Value::String("1.0.0".into()));
        pl.insert(
            "CFBundleShortVersionString".into(),
            Value::String("1.0.0".into()),
        );

        if let Some(icon) = &self.item.base.icon {
            // TODO remove unwrap
            let icon_name = Path::new(&icon).file_name().unwrap().to_str().unwrap();
            pl.insert("CFBundleIconFile".into(), Value::String(icon_name.into()));
        }

        if self.needs_appkit_launcher() {
            println!(
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
                env_dict.insert(k.into(), Value::String(v.into()));
            }
            pl.insert("LSEnvironment".into(), Value::Dictionary(env_dict));
        }

        if let Some(version) = self.item.ls_minimum_system_version.as_ref() {
            pl.insert(
                "LSMinimumSystemVersion".into(),
                Value::String(version.clone()),
            );
        }

        if let Some(prohibited) = self.item.ls_multiple_instances_prohibited {
            pl.insert(
                "LSMultipleInstancesProhibited".into(),
                Value::Boolean(prohibited),
            );
        }

        if let Some(requires_native) = self.item.ls_requires_native_execution {
            pl.insert(
                "LSRequiresNativeExecution".into(),
                Value::Boolean(requires_native),
            );
        }

        if let Some(supports) = self.item.ns_supports_automatic_graphics_switching {
            pl.insert(
                "NSSupportsAutomaticGraphicsSwitching".into(),
                Value::Boolean(supports),
            );
        }

        self.item
            .ut_exported_type_declarations
            .as_ref()
            .map(|_types| {
                // let mut type_array = Vec::new();
                // for t in types {
                //     let mut type_dict = plist::Dictionary::new();
                //     type_dict.insert("UTTypeConformsTo".into(), Value::Array(t.ut_type_conforms_to.iter().map(|s| Value::String(s.clone())).collect()));
                //     type_dict.insert("UTTypeDescription".into(), Value::String(t.ut_type_description.clone().unwrap_or_default()));
                //     type_dict.insert("UTTypeIconFile".into(), Value::String(t.ut_type_icon_file.clone().unwrap_or_default()));
                //     type_dict.insert("UTTypeIdentifier".into(), Value::String(t.ut_type_identifier.clone()));
                //     type_dict.insert("UTTypeReferenceURL".into(), Value::String(t.ut_type_reference_url.clone().unwrap_or_default()));
                //     let mut tag_spec = plist::Dictionary::new();
                //     for (k, v) in &t.ut_type_tag_specification {
                //         tag_spec.insert(k.clone(), Value::Array(v.iter().map(|s| Value::String(s.clone())).collect()));
                //     }
                //     type_dict.insert("UTTypeTagSpecification".into(), Value::Dictionary(tag_spec));
                //     type_array.push(Value::Dictionary(type_dict));
                // }
                // pl.insert("UTExportedTypeDeclarations".into(), Value::Array(type_array));
            });

        // self.item
        //     .ut_imported_type_declarations
        //     .as_ref()
        //     .map(|_types| {
        //         // let mut type_array = Vec::new();
        //         // for t in types {
        //         //     let mut type_dict = plist::Dictionary::new();
        //         //     type_dict.insert("UTTypeConformsTo".into(), Value::Array(t.ut_type_conforms_to.iter().map(|s| Value::String(s.clone())).collect()));
        //         //     type_dict.insert("UTTypeDescription".into(), Value::String(t.ut_type_description.clone().unwrap_or_default()));
        //         //     type_dict.insert("UTTypeIconFile".into(), Value::String(t.ut_type_icon_file.clone().unwrap_or_default()));
        //         //     type_dict.insert("UTTypeIdentifier".into(), Value::String(t.ut_type_identifier.clone()));
        //         //     type_dict.insert("UTTypeReferenceURL".into(), Value::String(t.ut_type_reference_url.clone().unwrap_or_default()));
        //         //     let mut tag_spec = plist::Dictionary::new();
        //         //     for (k, v) in &t.ut_type_tag_specification {
        //         //         tag_spec.insert(k.clone(), Value::Array(v.iter().map(|s| Value::String(s.clone())).collect()));
        //         //     }
        //         //     type_dict.insert("UTTypeTagSpecification".into(), Value::Dictionary(tag_spec));
        //         //     type_array.push(Value::Dictionary(type_dict));
        //         // }
        //         // pl.insert("UTImportedTypeDeclarations".into(), Value::Array(type_array));
        //     });

        println!(
            "Writing plist to {}",
            self.directories
                .location
                .join("Contents/Info.plist")
                .display()
        );
        plist::to_file_xml(self.directories.location.join("Contents/Info.plist"), &pl)?;

        Ok(())
    }

    fn sign_with_entitlements(&self) -> Result<(), MenuInstError> {
        // write a plist file with the entitlements to the filesystem
        let mut entitlements = plist::Dictionary::new();
        if let Some(entitlements_list) = &self.item.entitlements {
            for e in entitlements_list {
                let parts: Vec<&str> = e.split('=').collect();
                entitlements.insert(parts[0].to_string(), Value::String(parts[1].to_string()));
            }
        } else {
            return Ok(());
        }

        let entitlements_file = self
            .directories
            .location
            .join("Contents/Entitlements.plist");
        let writer = BufWriter::new(File::create(&entitlements_file)?);
        plist::to_writer_xml(writer, &entitlements)?;

        // sign the .app directory with the entitlements
        let _codesign = std::process::Command::new("codesign")
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
            .arg(self.directories.location.to_str().unwrap())
            .output()?;

        Ok(())
    }

    fn command(&self) -> String {
        let mut lines = vec!["#!/bin/sh".to_string()];

        if self.item.base.terminal.unwrap_or(false) {
            lines.extend_from_slice(&[
                r#"if [ "${__CFBundleIdentifier:-}" != "com.apple.Terminal" ]; then"#.to_string(),
                r#"    open -b com.apple.terminal "$0""#.to_string(),
                r#"    exit $?"#.to_string(),
                "fi".to_string(),
            ]);
        }

        if let Some(working_dir) = &self.item.base.working_dir {
            fs::create_dir_all(working_dir).expect("Failed to create working directory");
            lines.push(format!(r#"cd "{working_dir}""#));
        }

        if let Some(precommand) = &self.item.base.precommand {
            lines.push(precommand.clone());
        }

        // if self.item.base.activate {
        //     // Assuming these fields exist in your MacOS struct
        //     let conda_exe = &self.item.conda_exe;
        //     let prefix = &self.item.prefix;
        //     let activate = if self.is_micromamba(conda_exe) {
        //         "shell activate"
        //     } else {
        //         "shell.bash activate"
        //     };
        //     lines.push(format!(r#"eval "$("{}" {} "{}")""#, conda_exe, activate, prefix));
        // }

        lines.push(utils::quote_args(&self.item.base.command).join(" "));

        lines.join("\n")
    }

    fn write_appkit_launcher(&self) -> Result<PathBuf, MenuInstError> {
        // let launcher_path = launcher_path.unwrap_or_else(|| self.default_appkit_launcher_path());
        #[cfg(target_arch = "aarch64")]
        let launcher_bytes = include_bytes!("../data/appkit_launcher_arm64");
        #[cfg(target_arch = "x86_64")]
        let launcher_bytes = include_bytes!("../data/appkit_launcher_x86_64");

        let launcher_path = self.default_appkit_launcher_path();
        let mut file = File::create(&launcher_path)?;
        file.write_all(launcher_bytes)?;
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
        let script_path =
            script_path.unwrap_or_else(|| self.default_launcher_path().with_extension("script"));
        let mut file = File::create(&script_path)?;
        file.write_all(self.command().as_bytes())?;
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
            Some(logic) => logic,
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

    /// Find the launcher in the menuinst data path
    fn find_launcher(&self) -> Result<PathBuf, MenuInstError> {
        let launcher_name = format!("osx_launcher_{}", std::env::consts::ARCH);
        for datapath in utils::menuinst_data_paths(&self.prefix) {
            let launcher_path = datapath.join(&launcher_name);
            if launcher_path.is_file()
                && launcher_path.metadata()?.permissions().mode() & 0o111 != 0
            {
                return Ok(launcher_path);
            }
        }
        Err(MenuInstError::InstallError(format!(
            "Could not find executable launcher for {}",
            std::env::consts::ARCH
        )))
    }

    /// Find the appkit launcher in the menuinst data path
    fn find_appkit_launcher(&self) -> Result<PathBuf, MenuInstError> {
        let launcher_name = format!("appkit_launcher_{}", std::env::consts::ARCH);
        for datapath in utils::menuinst_data_paths(&self.prefix) {
            let launcher_path = datapath.join(&launcher_name);
            if launcher_path.is_file()
                && launcher_path.metadata()?.permissions().mode() & 0o111 != 0
            {
                return Ok(launcher_path);
            }
        }
        Err(MenuInstError::InstallError(format!(
            "Could not find executable appkit launcher for {}",
            std::env::consts::ARCH
        )))
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

    fn maybe_register_with_launchservices(&self, register: bool) -> Result<(), MenuInstError> {
        if !self.needs_appkit_launcher() {
            return Ok(());
        }

        if register {
            Self::lsregister(&["-R", self.directories.location.to_str().unwrap()])
        } else {
            Self::lsregister(&[
                "-R",
                "-u",
                "-all",
                self.directories.location.to_str().unwrap(),
            ])
        }
    }

    fn lsregister(args: &[&str]) -> Result<(), MenuInstError> {
        let exe = "/System/Library/Frameworks/CoreServices.framework/Frameworks/LaunchServices.framework/Support/lsregister";

        let output = Command::new(exe).args(args).output().map_err(|e| {
            MenuInstError::InstallError(format!("Failed to execute lsregister: {e}"))
        })?;

        if !output.status.success() {
            return Err(MenuInstError::InstallError(format!(
                "lsregister failed with exit code: {}",
                output.status
            )));
        }

        Ok(())
    }

    pub fn install(&self) -> Result<(), MenuInstError> {
        self.directories
            .create_directories(self.needs_appkit_launcher())?;
        self.install_icon()?;
        self.write_pkg_info()?;
        self.write_plist_info()?;
        self.write_appkit_launcher()?;
        self.write_launcher()?;
        self.write_script(None)?;
        self.write_event_handler(None)?;
        self.maybe_register_with_launchservices(true)?;

        Ok(())
    }

    pub fn remove(&self) -> Result<Vec<PathBuf>, MenuInstError> {
        println!("Removing {}", self.directories.location.display());
        self.maybe_register_with_launchservices(false)?;
        fs::remove_dir_all(&self.directories.location).unwrap_or_else(|e| {
            println!("Failed to remove directory: {e}. Ignoring error.");
        });
        Ok(vec![self.directories.location.clone()])
    }
}

pub(crate) fn install_menu_item(
    prefix: &Path,
    macos_item: MacOS,
    menu_mode: MenuMode,
) -> Result<(), MenuInstError> {
    let bundle_name = macos_item.cf_bundle_name.as_ref().unwrap();
    let directories = Directories::new(menu_mode, bundle_name);
    println!("Installing menu item for {bundle_name}");
    let menu = MacOSMenu::new(prefix, macos_item, directories);
    menu.install()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_slugify() {
        assert_eq!(slugify("Hello, World!"), "hello-world");
    }
}
