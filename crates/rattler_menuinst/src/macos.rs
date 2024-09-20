use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
};

use crate::{schema::MacOS, slugify, MenuInstError, MenuMode};
use plist::Value;
use sha1::{Digest, Sha1};

pub struct MacOSMenu {
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
    pub fn new(item: MacOS, directories: Directories) -> Self {
        Self { item, directories }
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
            fs::copy(&icon, dest)?;

            if self.needs_appkit_launcher() {
                let dest = self.directories.nested_resources().join(icon_name);
                fs::copy(&icon, dest)?;
            }
        }

        Ok(())
    }

    fn write_pkg_info(&self) -> Result<(), MenuInstError> {
        let create_pkg_info = |path: &PathBuf, short_name: &str| -> Result<(), MenuInstError> {
            let mut f = fs::File::create(path.join("Contents/PkgInfo"))?;
            f.write_all(format!("APPL{short_name}").as_bytes())?;
            Ok(())
        };
        let short_name = slugify(&self.item.base.name.chars().take(8).collect::<String>());

        create_pkg_info(&self.directories.location, &short_name)?;
        if self.needs_appkit_launcher() {
            create_pkg_info(&self.directories.nested_location, &short_name)?;
        }

        Ok(())
    }

    fn write_plist(&self) -> Result<(), MenuInstError> {
        let name = self.item.base.name.clone();
        let slugname = slugify(&name);

        let shortname = if slugname.len() > 16 {
            let hashed = format!("{:x}", Sha1::digest(slugname.as_bytes()));
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
            Value::String(format!("{}-1.0.0", slugname)),
        );
        pl.insert(
            "CFBundleIdentifier".into(),
            Value::String(format!("com.{}", slugname)),
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
            plist::to_file_xml(
                self.directories.nested_location.join("Contents/Info.plist"),
                &pl,
            )?;
            pl.insert("LSBackgroundOnly".into(), Value::Boolean(true));
            pl.insert(
                "CFBundleIdentifier".into(),
                Value::String(format!("com.{}-appkit-launcher", slugname)),
            );
        }

        plist::to_file_xml(self.directories.location.join("Contents/Info.plist"), &pl)?;

        Ok(())
    }

    pub fn install(&self) -> Result<(), MenuInstError> {
        self.directories
            .create_directories(self.needs_appkit_launcher())?;
        self.install_icon()?;
        self.write_pkg_info()?;
        Ok(())
    }
}

pub(crate) fn install_menu_item(
    macos_item: MacOS,
    menu_mode: MenuMode,
) -> Result<(), MenuInstError> {
    let bundle_name = macos_item.cf_bundle_name.as_ref().unwrap();
    let directories = Directories::new(menu_mode, bundle_name);

    let menu = MacOSMenu::new(macos_item, directories);
    menu.install()
}
