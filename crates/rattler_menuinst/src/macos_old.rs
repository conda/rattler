use std::collections::HashMap;
use std::fs::{self, File};
use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;

use plist::Value;
use sha1::{Digest, Sha1};
use tracing;

use crate::utils::unix_lex::UnixLex;

// use crate::utils::{UnixLex, logged_run};
// use crate::base::{Menu, MenuItem, menuitem_defaults};

pub struct MacOSMenu {
    name: String,
    mode: String,
    prefix: PathBuf,
}

impl MacOSMenu {
    pub fn new(name: String, mode: String, prefix: PathBuf) -> Self {
        MacOSMenu { name, mode, prefix }
    }

    pub fn create(&self) -> Vec<PathBuf> {
        self.paths()
    }

    pub fn remove(&self) -> Vec<PathBuf> {
        self.paths()
    }

    pub fn placeholders(&self) -> HashMap<String, String> {
        let mut placeholders = HashMap::new();
        placeholders.insert(
            "SP_DIR".to_string(),
            self.site_packages().to_str().unwrap().to_string(),
        );
        placeholders.insert("ICON_EXT".to_string(), "icns".to_string());
        placeholders.insert(
            "PYTHONAPP".to_string(),
            self.prefix
                .join("python.app/Contents/MacOS/python")
                .to_str()
                .unwrap()
                .to_string(),
        );
        placeholders
    }

    fn paths(&self) -> Vec<PathBuf> {
        vec![]
    }

    fn site_packages(&self) -> PathBuf {
        // Implement site-packages directory detection logic here
        self.prefix.join("lib/python3.x/site-packages")
    }
}

pub struct MacOSMenuItem {
    menu: MacOSMenu,
    metadata: HashMap<String, Value>,
}

impl MacOSMenuItem {
    pub fn new(menu: MacOSMenu, metadata: HashMap<String, Value>) -> Self {
        MacOSMenuItem { menu, metadata }
    }

    pub fn location(&self) -> PathBuf {
        self.base_location()
            .join("Applications")
            .join(self.bundle_name())
    }

    fn bundle_name(&self) -> String {
        return "foo.app".to_string();
        // format!("{}.app", self.render_key("name", &HashMap::new()))
    }

    fn nested_location(&self) -> PathBuf {
        self.location()
            .join("Contents/Resources")
            .join(self.bundle_name())
    }

    fn base_location(&self) -> PathBuf {
        // if self.menu.mode == "user" {
        //     // PathBuf::from(shellexpand::tilde("~").to_string())
        // } else {
        //     PathBuf::from("/")
        // }
        PathBuf::from("/FOO")
    }

    pub fn create(&self) -> Vec<PathBuf> {
        if self.location().exists() {
            panic!("App already exists at {:?}. Please remove the existing shortcut before installing.", self.location());
        }
        tracing::debug!("Creating {:?}", self.location());
        self.create_application_tree();
        // self.precreate();
        // self.copy_icon();
        // self.write_pkginfo();
        // self.write_plistinfo();
        // self.write_appkit_launcher();
        // self.write_launcher();
        // self.write_script();
        // self.write_event_handler();
        // self.maybe_register_with_launchservices();
        self.sign_with_entitlements();
        vec![self.location()]
    }

    pub fn remove(&self) -> Vec<PathBuf> {
        tracing::debug!("Removing {:?}", self.location());
        self.maybe_register_with_launchservices(false);
        fs::remove_dir_all(&self.location()).unwrap();
        vec![self.location()]
    }

    fn create_application_tree(&self) -> Vec<PathBuf> {
        let mut paths = vec![
            self.location().join("Contents/Resources"),
            self.location().join("Contents/MacOS"),
        ];
        if self.needs_appkit_launcher() {
            paths.push(self.nested_location().join("Contents/Resources"));
            paths.push(self.nested_location().join("Contents/MacOS"));
        }
        for path in &paths {
            fs::create_dir_all(path).unwrap();
        }
        paths
    }

    fn copy_icon(&self) {
        if let Some(icon) = self.render_key("icon", &HashMap::new()) {
            fs::copy(
                &icon,
                self.location()
                    .join("Contents/Resources")
                    .join(Path::new(&icon).file_name().unwrap()),
            )
            .unwrap();
            if self.needs_appkit_launcher() {
                fs::copy(
                    &icon,
                    self.nested_location()
                        .join("Contents/Resources")
                        .join(Path::new(&icon).file_name().unwrap()),
                )
                .unwrap();
            }
        }
    }

    fn write_pkginfo(&self) {
        let app_bundles = if self.needs_appkit_launcher() {
            vec![self.location(), self.nested_location()]
        } else {
            vec![self.location()]
        };
        for app in app_bundles {
            let mut file = File::create(app.join("Contents/PkgInfo")).unwrap();
            write!(file, "APPL{}", &self.menu.name.to_lowercase()[..8]).unwrap();
        }
    }

    fn render(self, value: &str) -> String {
        // Implement rendering logic here
        value.to_string()

    }

    fn write_plistinfo(&self) {
        let name = self.menu.name.clone();
        // let slugname = self.render_key("name", &[("slug", &true.into())].iter().cloned().collect());
        let name = "foo".to_string();
        let slugname = "foo".to_string();
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
        pl.insert("CFBundleGetInfoString".into(), Value::String(format!("{}-1.0.0", slugname)));
        pl.insert("CFBundleIdentifier".into(), Value::String(format!("com.{}", slugname)));
        pl.insert("CFBundlePackageType".into(), Value::String("APPL".into()));
        pl.insert("CFBundleVersion".into(), Value::String("1.0.0".into()));
        pl.insert("CFBundleShortVersionString".into(), Value::String("1.0.0".into()));

        let icon = self.render_key("icon", &HashMap::new());

        // if let Some(icon) = self.render_key("icon", &HashMap::new()) {
        //     pl.insert("CFBundleIconFile".into(), Value::String(Path::new(&icon).file_name().unwrap().to_str().unwrap().into()));
        // }

        if self.needs_appkit_launcher() {
            plist::to_file_xml(self.nested_location().join("Contents/Info.plist"), &pl).unwrap();
            pl.insert("LSBackgroundOnly".into(), Value::Boolean(true));
            pl.insert("CFBundleIdentifier".into(), Value::String(format!("com.{}-appkit-launcher", slugname)));
        }

        // Override defaults with user provided values
        // for key in menuitem_defaults["platforms"]["osx"].keys() {
        //     if menuitem_defaults.contains_key(key) || key == "entitlements" || key == "link_in_bundle" {
        //         continue;
        //     }
        //     if let Some(value) = self.render_key(key, &HashMap::new()) {
        //         if key == "CFBundleVersion" {
        //             pl.insert("CFBundleShortVersionString".into(), Value::String(value.clone()));
        //             pl.insert("CFBundleGetInfoString".into(), Value::String(format!("{}-{}", slugname, value)));
        //         }
        //         pl.insert(key.into(), Value::String(value));
        //     }
        // }

        plist::to_file_xml(self.location().join("Contents/Info.plist"), &pl).unwrap();
    }

    fn command(&self) -> String {
        let mut lines = vec!["#!/bin/sh".to_string()];
        if self.render_key("terminal", &HashMap::new()) == Some("true".to_string()) {
            lines.extend_from_slice(&[
                r#"if [ "${__CFBundleIdentifier:-}" != "com.apple.Terminal" ]; then"#.to_string(),
                r#"    open -b com.apple.terminal "$0""#.to_string(),
                r#"    exit $?"#.to_string(),
                "fi".to_string(),
            ]);
        }

        // if let Some(working_dir) = self.render_key("working_dir", &HashMap::new()) {
        //     fs::create_dir_all(shellexpand::full(&working_dir).unwrap().to_string()).unwrap();
        //     lines.push(format!(r#"cd "{}""#, working_dir));
        // }

        if let Some(precommand) = self.render_key("precommand", &HashMap::new()) {
            lines.push(precommand);
        }

        if self.metadata.get("activate") == Some(&Value::Boolean(true)) {
            let conda_exe = &self.menu.prefix.join("bin/conda");
            // let activate = if self.menu.is_micromamba(conda_exe) { "shell activate" } else { "shell.bash activate" };
            // lines.push(format!(r#"eval "$("{}" {} "{}")""#, conda_exe.display(), activate, self.menu.prefix.display()));
        }

        // lines.push(UnixLex::quote_args(&self.render_key("command", &HashMap::new()).unwrap()).join(" "));

        lines.join("\n")
    }

    fn default_appkit_launcher_path(&self) -> PathBuf {
        self.location()
            .join("Contents/MacOS")
            .join(self.bundle_name())
    }

    fn default_launcher_path(&self, suffix: Option<&str>) -> PathBuf {
        let suffix = suffix.unwrap_or("");
        let name = self.render_key("name", &HashMap::new()).unwrap();
        if self.needs_appkit_launcher() {
            return self
                .nested_location()
                .join("Contents/MacOS")
                .join(format!("{name}{suffix}"));
        }
        self.location()
            .join("Contents/MacOS")
            .join(format!("{name}{suffix}"))
    }

    fn write_appkit_launcher(&self, launcher_path: Option<&Path>) -> PathBuf {
        let launcher_path = launcher_path
            .map(PathBuf::from)
            .unwrap_or_else(|| self.default_appkit_launcher_path());
        // fs::copy(self.find_appkit_launcher(), launcher_path).unwrap();
        // fs::set_permissions(launcher_path, fs::Permissions::from_mode(0o755)).unwrap();
        launcher_path.to_path_buf()
    }

    fn write_launcher(&self, launcher_path: Option<&Path>) -> PathBuf {
        let launcher_path = launcher_path
            .map(PathBuf::from)
            .unwrap_or_else(|| self.default_launcher_path(None));
        // fs::copy(self.find_launcher(), launcher_path).unwrap();
        // fs::set_permissions(launcher_path, fs::Permissions::from_mode(0o755)).unwrap();
        launcher_path.to_path_buf()
    }

    fn write_script(&self, script_path: Option<&Path>) -> PathBuf {
        let script_path = script_path
            .map(PathBuf::from)
            .unwrap_or_else(|| self.default_launcher_path(None).with_extension("script"));
        fs::write(&script_path, self.command()).unwrap();
        fs::set_permissions(&script_path, fs::Permissions::from_mode(0o755)).unwrap();
        script_path
    }

    fn write_event_handler(&self, script_path: Option<&Path>) -> Option<PathBuf> {
        if !self.needs_appkit_launcher() {
            return None;
        }
        let event_handler_logic = self.render_key("event_handler", &HashMap::new())?;
        let script_path = script_path
            .map(PathBuf::from)
            .unwrap_or_else(|| self.location().join("Contents/Resources/handle-event"));
        fs::write(
            &script_path,
            format!("#!/bin/bash\n{}\n", event_handler_logic),
        )
        .unwrap();
        fs::set_permissions(&script_path, fs::Permissions::from_mode(0o755)).unwrap();
        Some(script_path)
    }

    fn maybe_register_with_launchservices(&self, register: bool) {
        if !self.needs_appkit_launcher() {
            return;
        }
        if register {
            lsregister(&["-R", self.location().to_str().unwrap()]);
        } else {
            lsregister(&["-R", "-u", "-all", self.location().to_str().unwrap()]);
        }
    }

    fn sign_with_entitlements(&self) {
        let entitlements = Vec::<String>::new();
        if entitlements.is_empty() {
            return;
        }

        let mut plist = plist::Dictionary::new();
        for key in entitlements {
            plist.insert(key, Value::Boolean(true));
        }

        let slugname = self.render_key("name", &[("slug", &true.into())].iter().cloned().collect());
        let entitlements_path = self.location().join("Contents/Entitlements.plist");
        plist::to_file_xml(&entitlements_path, &plist).unwrap();

        // logged_run(
        //     &[
        //         "/usr/bin/codesign",
        //         "--verbose",
        //         "--sign",
        //         "-",
        //         "--prefix",
        //         &format!("com.{}", slugname),
        //         "--options",
        //         "runtime",
        //         "--force",
        //         "--deep",
        //         "--entitlements",
        //         entitlements_path.to_str().unwrap(),
        //         self.location().to_str().unwrap(),
        //     ],
        //     true,
        // ).unwrap();
        }
    }

    fn needs_appkit_launcher(&self) -> bool {
        self.metadata.get("event_handler").is_some()
    }

    fn render_key(&self, key: &str, extra: &HashMap<&str, Value>) -> Option<String> {
        // Implement rendering logic here
        self.metadata
            .get(key)
            .and_then(|v| v.as_string().map(|s| s.to_string()))
    }

    // Implement other helper methods...
}

fn lsregister(args: &[&str]) {
    let exe = "/System/Library/Frameworks/CoreServices.framework/Frameworks/LaunchServices.framework/Support/lsregister";
    Command::new(exe)
        .args(args)
        .output()
        .expect("Failed to execute lsregister");
}
