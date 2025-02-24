use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::render::{BaseMenuItemPlaceholders, PlaceholderString};

#[derive(Serialize, Deserialize, Debug)]
#[serde(deny_unknown_fields)]
pub struct MenuItemNameDict {
    target_environment_is_base: Option<String>,
    target_environment_is_not_base: Option<String>,
}

/// A platform-specific menu item configuration.
///
/// This is equivalent to `MenuItem` but without `platforms` field and all fields are optional.
/// All fields default to `None`.
#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(deny_unknown_fields)]
pub struct BasePlatformSpecific {
    /// The name of the menu item.
    ///
    /// Must be at least 1 character long.
    pub name: Option<NameField>,

    /// A longer description of the menu item.
    ///
    /// Displayed in popup messages.
    pub description: Option<PlaceholderString>,

    /// Path to the file representing or containing the icon.
    ///
    /// Must be at least 1 character long.
    pub icon: Option<PlaceholderString>,

    /// Command to run with the menu item.
    ///
    /// Represented as a list of strings where each string is an argument.
    /// Must contain at least one item.
    pub command: Option<Vec<PlaceholderString>>,

    /// Working directory for the running process.
    ///
    /// Defaults to user directory on each platform.
    /// Must be at least 1 character long.
    pub working_dir: Option<PlaceholderString>,

    /// Logic to run before the command is executed.
    ///
    /// Runs before the env is activated, if applicable.
    /// Should be simple, preferably single-line.
    pub precommand: Option<PlaceholderString>,

    /// Logic to run before the shortcut is created.
    ///
    /// Should be simple, preferably single-line.
    pub precreate: Option<PlaceholderString>,

    /// Whether to activate the target environment before running `command`.
    pub activate: Option<bool>,

    /// Whether to run the program in a terminal/console.
    ///
    /// ### Platform-specific behavior
    /// - `Windows`: Only has an effect if `activate` is true
    /// - `MacOS`: The application will ignore command-line arguments
    pub terminal: Option<bool>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(deny_unknown_fields)]
pub struct Platform<T> {
    #[serde(flatten)]
    pub base: BasePlatformSpecific,
    #[serde(flatten)]
    pub specific: T,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(untagged)]
pub enum NameField {
    Simple(PlaceholderString),
    Complex(NameComplex),
}

impl NameField {
    pub fn resolve(&self, env: Environment, placeholders: &BaseMenuItemPlaceholders) -> String {
        match self {
            NameField::Simple(name) => name.resolve(placeholders),
            NameField::Complex(complex_name) => match env {
                Environment::Base => complex_name
                    .target_environment_is_base
                    .resolve(placeholders),
                Environment::NotBase => complex_name
                    .target_environment_is_not_base
                    .resolve(placeholders),
            },
        }
    }
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct NameComplex {
    pub target_environment_is_base: PlaceholderString,
    pub target_environment_is_not_base: PlaceholderString,
}

pub enum Environment {
    Base,
    #[allow(dead_code)]
    NotBase,
}

/// Windows-specific instructions for menu item configuration.
///
/// Allows overriding global keys for Windows-specific behavior.
#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(deny_unknown_fields)]
pub struct Windows {
    /// Whether to create a desktop icon in addition to the Start Menu item.
    ///
    /// Defaults to `true` in the original implementation.
    pub desktop: Option<bool>,

    /// Whether to create a quick launch icon in addition to the Start Menu item.
    ///
    /// Defaults to `true` in the original implementation.
    pub quicklaunch: Option<bool>,

    /// Windows Terminal profile configuration.
    pub terminal_profile: Option<PlaceholderString>,

    /// URL protocols that will be associated with this program.
    ///
    /// Each protocol must contain no whitespace characters.
    pub url_protocols: Option<Vec<PlaceholderString>>,

    /// File extensions that will be associated with this program.
    ///
    /// Each extension must start with a dot and contain no whitespace.
    pub file_extensions: Option<Vec<PlaceholderString>>,

    /// Application User Model ID for Windows 7 and above.
    ///
    /// Used to associate processes, files and windows with a particular application.
    /// Required when shortcut produces duplicated icons.
    ///
    /// # Format
    /// - Must contain at least two segments separated by dots
    /// - Maximum length of 128 characters
    /// - No whitespace allowed
    ///
    /// # Default
    /// If not set, defaults to `Menuinst.<name>`
    ///
    /// For more information, see [Microsoft's AppUserModelID documentation](https://learn.microsoft.com/en-us/windows/win32/shell/appids#how-to-form-an-application-defined-appusermodelid)
    pub app_user_model_id: Option<PlaceholderString>,
}

/// Linux-specific instructions.
/// Check the `Desktop entry specification <https://specifications.freedesktop.org/desktop-entry-spec/desktop-entry-spec-latest.html#recognized-keys>` for more details.
#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(deny_unknown_fields)]
pub struct Linux {
    /// Categories in which the entry should be shown in a menu.
    /// See 'Registered categories' in the `Menu Spec <http://www.freedesktop.org/Standards/menu-spec>`.
    #[serde(rename = "Categories")]
    pub categories: Option<Vec<PlaceholderString>>,

    /// A boolean value specifying if D-Bus activation is supported for this application.
    #[serde(rename = "DBusActivatable")]
    pub dbus_activatable: Option<bool>,

    /// Generic name of the application; e.g. if the name is 'conda',
    /// this would be 'Package Manager'.
    #[serde(rename = "GenericName")]
    pub generic_name: Option<PlaceholderString>,

    /// Disable shortcut, signaling a missing resource.
    #[serde(rename = "Hidden")]
    pub hidden: Option<bool>,

    /// List of supported interfaces. See 'Interfaces' in
    /// `Desktop Entry Spec <https://specifications.freedesktop.org/desktop-entry-spec/desktop-entry-spec-latest.html#interfaces>`.
    #[serde(rename = "Implements")]
    pub implements: Option<Vec<PlaceholderString>>,

    /// Additional terms to describe this shortcut to aid in searching.
    #[serde(rename = "Keywords")]
    pub keywords: Option<Vec<PlaceholderString>>,

    /// Do not show the 'New Window' option in the app's context menu.
    #[serde(rename = "SingleMainWindow")]
    pub single_main_window: Option<bool>,

    /// The MIME type(s) supported by this application. Note this includes file
    /// types and URL protocols. For URL protocols, use
    /// `x-scheme-handler/your-protocol-here`. For example, if you want to
    /// register `menuinst:`, you would include `x-scheme-handler/menuinst`.
    #[serde(rename = "MimeType")]
    pub mime_type: Option<Vec<PlaceholderString>>,

    /// Do not show this item in the menu. Useful to associate MIME types
    /// and other registrations, without having an actual clickable item.
    /// Not to be confused with 'Hidden'.
    #[serde(rename = "NoDisplay")]
    pub no_display: Option<bool>,

    /// Desktop environments that should NOT display this item.
    /// It'll check against `$XDG_CURRENT_DESKTOP`."
    #[serde(rename = "NotShowIn")]
    pub not_show_in: Option<Vec<PlaceholderString>>,

    /// Desktop environments that should display this item.
    /// It'll check against `$XDG_CURRENT_DESKTOP`.
    #[serde(rename = "OnlyShowIn")]
    pub only_show_in: Option<Vec<PlaceholderString>>,

    /// Hint that the app prefers to be run on a more powerful discrete GPU if available.
    #[serde(rename = "PrefersNonDefaultGPU")]
    pub prefers_non_default_gpu: Option<bool>,

    /// Advanced. See `Startup Notification spec <https://www.freedesktop.org/wiki/Specifications/startup-notification-spec/>`.
    #[serde(rename = "StartupNotify")]
    pub startup_notify: Option<bool>,

    /// Advanced. See `Startup Notification spec <https://www.freedesktop.org/wiki/Specifications/startup-notification-spec/>`.
    #[serde(rename = "StartupWMClass")]
    pub startup_wm_class: Option<PlaceholderString>,

    /// Filename or absolute path to an executable file on disk used to
    /// determine if the program is actually installed and can be run. If the test
    /// fails, the shortcut might be ignored / hidden.
    #[serde(rename = "TryExec")]
    pub try_exec: Option<PlaceholderString>,

    /// Map of custom MIME types to their corresponding glob patterns (e.g. `*.txt`).
    /// Only needed if you define custom MIME types in `MimeType`.
    pub glob_patterns: Option<HashMap<PlaceholderString, PlaceholderString>>,
}

// Enum for CFBundleTypeRole with validation
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub enum CFBundleTypeRole {
    /// The app is the creator/owner of this file type
    Editor,
    /// The app is a viewer of this file type
    Viewer,
    /// The app is a shell for this file type
    Shell,
    /// Quick Look Generator
    QLGenerator,
    /// The app is not a handler for this file type
    None,
}

/// Describes a URL scheme associated with the app.
#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(deny_unknown_fields)]
pub struct CFBundleURLTypesModel {
    /// This key specifies the app's role with respect to the URL.
    /// Can be one of `Editor`, `Viewer`, `Shell`, `None`
    #[serde(rename = "CFBundleTypeRole")]
    pub cf_bundle_type_role: Option<CFBundleTypeRole>,

    /// URL schemes / protocols handled by this type (e.g. 'mailto').
    #[serde(rename = "CFBundleURLSchemes")]
    pub cf_bundle_url_schemes: Vec<PlaceholderString>,

    /// Abstract name for this URL type. Uniqueness recommended.
    #[serde(rename = "CFBundleURLName")]
    pub cf_bundle_url_name: PlaceholderString,

    /// Name of the icon image file (minus the .icns extension).
    #[serde(rename = "CFBundleURLIconFile")]
    pub cf_bundle_url_icon_file: Option<PlaceholderString>,
}

// Enum for LSHandlerRank with validation
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub enum LSHandlerRank {
    /// The app is the creator/owner of this file type
    Owner,
    /// The app is the default handler for this file type
    Default,
    /// The app is an alternate handler for this file type
    Alternate,
    /// The app is not a handler for this file type
    None,
}

/// Describes a document type associated with the app.
#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(deny_unknown_fields)]
pub struct CFBundleDocumentTypesModel {
    /// Abstract name for this document type. Uniqueness recommended.
    #[serde(rename = "CFBundleTypeName")]
    pub cf_bundle_type_name: PlaceholderString,

    /// Name of the icon image file (minus the .icns extension).
    #[serde(rename = "CFBundleTypeIconFile")]
    pub cf_bundle_type_icon_file: Option<PlaceholderString>,

    /// This key specifies the app's role with respect to the type.
    /// Can be one of `Editor`, `Viewer`, `Shell`, `None`
    #[serde(rename = "CFBundleTypeRole")]
    pub cf_bundle_type_role: Option<CFBundleTypeRole>,

    /// List of UTI (Uniform Type Identifier) strings defining supported file types.
    ///
    /// # Examples
    ///
    /// For PNG files, use `public.png`
    ///
    /// # Details
    ///
    /// - System-defined UTIs can be found in the [UTI Reference](https://developer.apple.com/library/archive/documentation/Miscellaneous/Reference/UTIRef/Articles/System-DeclaredUniformTypeIdentifiers.html)
    /// - Custom UTIs can be defined via `UTExportedTypeDeclarations`
    /// - UTIs from other apps must be imported via `UTImportedTypeDeclarations`
    ///
    /// For more information, see the [Fun with UTIs](https://www.cocoanetics.com/2012/09/fun-with-uti/) guide.
    #[serde(rename = "LSItemContentTypes")]
    pub ls_item_content_types: Vec<PlaceholderString>,

    /// Determines how Launch Services ranks this app among the apps
    /// that declare themselves editors or viewers of files of this type.
    /// Can be one of `Owner`, `Default`, `Alternate` or `None`
    #[serde(rename = "LSHandlerRank")]
    pub ls_handler_rank: LSHandlerRank,
}

/// A model representing a Uniform Type Identifier (UTI) declaration.
#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(deny_unknown_fields)]
pub struct UTTypeDeclarationModel {
    /// The Uniform Type Identifier types that this type conforms to.
    #[serde(rename = "UTTypeConformsTo")]
    pub ut_type_conforms_to: Vec<PlaceholderString>,

    /// A description for this type.
    #[serde(rename = "UTTypeDescription")]
    pub ut_type_description: Option<PlaceholderString>,

    /// The bundle icon resource to associate with this type.
    #[serde(rename = "UTTypeIconFile")]
    pub ut_type_icon_file: Option<PlaceholderString>,

    /// The Uniform Type Identifier to assign to this type.
    #[serde(rename = "UTTypeIdentifier")]
    pub ut_type_identifier: PlaceholderString,

    /// The webpage for a reference document that describes this type.
    #[serde(rename = "UTTypeReferenceURL")]
    pub ut_type_reference_url: Option<PlaceholderString>,

    /// A dictionary defining one or more equivalent type identifiers.
    #[serde(rename = "UTTypeTagSpecification")]
    pub ut_type_tag_specification: HashMap<PlaceholderString, Vec<PlaceholderString>>,
}

/// macOS version number.
#[derive(Debug, Clone)]
pub struct MacOSVersion(pub(crate) Vec<u32>);

impl Serialize for MacOSVersion {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        self.0
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<String>>()
            .join(".")
            .serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for MacOSVersion {
    fn deserialize<D>(deserializer: D) -> Result<MacOSVersion, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let s = String::deserialize(deserializer)?;
        let version = s
            .split('.')
            .map(str::parse)
            .collect::<Result<Vec<u32>, _>>()
            .map_err(serde::de::Error::custom)?;

        if version.len() > 3 {
            return Err(serde::de::Error::custom(
                "Version number must have at most 3 components",
            ));
        }

        Ok(MacOSVersion(version))
    }
}
/// macOS specific fields in the menuinst. For more information on the keys, read the following URLs
///
/// - `CF*` keys: see `Core Foundation Keys <https://developer.apple.com/library/archive/documentation/General/Reference/InfoPlistKeyReference/Articles/CoreFoundationKeys.html>`
/// - `LS*` keys: see `Launch Services Keys <https://developer.apple.com/library/archive/documentation/General/Reference/InfoPlistKeyReference/Articles/LaunchServicesKeys.html>`
/// - `entitlements`: see `entitlements docs <https://developer.apple.com/documentation/bundleresources/entitlements>`
#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(deny_unknown_fields)]
pub struct MacOS {
    /// Display name of the bundle, visible to users and used by Siri. If
    /// not provided, 'menuinst' will use the 'name' field.
    #[serde(rename = "CFBundleDisplayName")]
    pub cf_bundle_display_name: Option<PlaceholderString>,

    /// Unique identifier for the shortcut. Typically uses a reverse-DNS format.
    /// If not provided, a identifier will be generated from the 'name' field.
    #[serde(rename = "CFBundleIdentifier")]
    pub cf_bundle_identifier: Option<PlaceholderString>,

    /// Short name of the bundle. May be used if `CFBundleDisplayName` is
    /// absent. If not provided, 'menuinst' will generate one from the 'name' field.
    #[serde(rename = "CFBundleName")]
    pub cf_bundle_name: Option<PlaceholderString>,

    /// Suitable replacement for text-to-speech operations on the app.
    /// For example, "my app one two three" instead of `MyApp123`.
    #[serde(rename = "CFBundleSpokenName")]
    pub cf_bundle_spoken_name: Option<PlaceholderString>,

    /// Build version number for the bundle. In the context of 'menuinst'
    /// this can be used to signal a new version of the menu item for the same
    /// application version.    
    #[serde(rename = "CFBundleVersion")]
    pub cf_bundle_version: Option<PlaceholderString>,

    /// URL types supported by this app. Requires setting `event_handler` too.
    /// Note this feature requires macOS 10.15+.    
    #[serde(rename = "CFBundleURLTypes")]
    pub cf_bundle_url_types: Option<Vec<CFBundleURLTypesModel>>,

    /// Document types supported by this app. Requires setting `event_handler` too.
    /// Requires macOS 10.15+.
    #[serde(rename = "CFBundleDocumentTypes")]
    pub cf_bundle_document_types: Option<Vec<CFBundleDocumentTypesModel>>,

    /// The App Store uses this string to determine the appropriate categorization.
    #[serde(rename = "LSApplicationCategoryType")]
    pub ls_application_category_type: Option<String>,

    /// Specifies whether this app runs only in the background
    #[serde(rename = "LSBackgroundOnly")]
    pub ls_background_only: Option<bool>,

    /// List of key-value pairs used to define environment variables.
    #[serde(rename = "LSEnvironment")]
    pub ls_environment: Option<HashMap<String, PlaceholderString>>,

    /// Minimum version of macOS required for this app to run, as `x.y.z`.
    /// For example, for macOS v10.4 and later, use `10.4.0`. (TODO: implement proper parsing)
    #[serde(rename = "LSMinimumSystemVersion")]
    pub ls_minimum_system_version: Option<MacOSVersion>,

    /// Whether an app is prohibited from running simultaneously in multiple user sessions.
    #[serde(rename = "LSMultipleInstancesProhibited")]
    pub ls_multiple_instances_prohibited: Option<bool>,

    /// If true, prevent a universal binary from being run under
    /// Rosetta emulation on an Intel-based Mac.
    #[serde(rename = "LSRequiresNativeExecution")]
    pub ls_requires_native_execution: Option<bool>,

    /// The uniform type identifiers owned and exported by the app.
    #[serde(rename = "UTExportedTypeDeclarations")]
    pub ut_exported_type_declarations: Option<Vec<UTTypeDeclarationModel>>,

    /// The uniform type identifiers inherently supported, but not owned, by the app.
    #[serde(rename = "UTImportedTypeDeclarations")]
    pub ut_imported_type_declarations: Option<Vec<UTTypeDeclarationModel>>,

    /// If true, allows an OpenGL app to utilize the integrated GPU.
    #[serde(rename = "NSSupportsAutomaticGraphicsSwitching")]
    pub ns_supports_automatic_graphics_switching: Option<bool>,

    /// List of permissions to request for the launched application.
    /// See `the entitlements docs <https://developer.apple.com/documentation/bundleresources/entitlements>`
    /// for a full list of possible values.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub entitlements: Option<Vec<String>>,

    /// Paths that should be symlinked into the shortcut app bundle.
    /// It takes a mapping of source to destination paths. Destination paths must be
    /// relative. Placeholder `{{ MENU_ITEM_LOCATION }}` can be useful.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub link_in_bundle: Option<HashMap<PlaceholderString, PlaceholderString>>,

    /// Required shell script logic to handle opened URL payloads.
    pub event_handler: Option<PlaceholderString>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(deny_unknown_fields)]
pub struct Platforms {
    pub linux: Option<Platform<Linux>>,
    pub osx: Option<Platform<MacOS>>,
    pub win: Option<Platform<Windows>>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(deny_unknown_fields)]
pub struct MenuItem {
    #[serde(flatten)]
    pub command: MenuItemCommand,
    pub platforms: Platforms,
}

/// Instructions to create a menu item across operating systems.
#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(deny_unknown_fields)]
pub struct MenuItemCommand {
    /// The name of the menu item.
    ///
    /// Must be at least 1 character long.
    pub name: NameField,

    /// A longer description of the menu item.
    ///
    /// Displayed in popup messages.
    pub description: PlaceholderString,

    /// Command to run with the menu item.
    ///
    /// Represented as a list of strings where each string is an argument.
    /// Must contain at least one item.
    pub command: Vec<PlaceholderString>,

    /// Path to the file representing or containing the icon.
    ///
    /// Must be at least 1 character long when provided.
    pub icon: Option<PlaceholderString>,

    /// Logic to run before the command is executed.
    ///
    /// Should be simple, preferably single-line.
    /// Runs before the environment is activated, if applicable.
    pub precommand: Option<PlaceholderString>,

    /// Logic to run before the shortcut is created.
    ///
    /// Should be simple, preferably single-line.
    pub precreate: Option<PlaceholderString>,

    /// Working directory for the running process.
    ///
    /// Defaults to user directory on each platform.
    /// Must be at least 1 character long when provided.
    pub working_dir: Option<PlaceholderString>,

    /// Whether to activate the target environment before running `command`.
    ///
    /// Defaults to `true` in the original implementation.
    pub activate: Option<bool>,

    /// Whether to run the program in a terminal/console.
    ///
    /// Defaults to `false` in the original implementation.
    ///
    /// # Platform-specific behavior
    /// - `Windows`: Only has an effect if `activate` is true
    /// - `MacOS`: The application will ignore command-line arguments
    pub terminal: Option<bool>,
}

impl MenuItemCommand {
    /// Merge the generic `MenuItemCommand` with a platform-specific `BasePlatformSpecific`.
    pub fn merge(&self, platform: BasePlatformSpecific) -> MenuItemCommand {
        MenuItemCommand {
            name: platform.name.unwrap_or_else(|| self.name.clone()),
            description: platform
                .description
                .unwrap_or_else(|| self.description.clone()),
            command: platform.command.unwrap_or_else(|| self.command.clone()),
            icon: platform.icon.as_ref().or(self.icon.as_ref()).cloned(),
            precommand: platform.precommand.or_else(|| self.precommand.clone()),
            precreate: platform.precreate.or_else(|| self.precreate.clone()),
            working_dir: platform.working_dir.or_else(|| self.working_dir.clone()),
            activate: platform.activate.or(self.activate),
            terminal: platform.terminal.or(self.terminal),
        }
    }
}

/// Metadata required to create menu items across operating systems with `menuinst`
#[derive(Serialize, Deserialize, Debug)]
pub struct MenuInstSchema {
    /// Standard of the JSON schema we adhere to.
    #[serde(rename = "$schema")]
    pub schema: String,

    /// Name for the category containing the items specified in `menu_items`.
    pub menu_name: String,

    /// List of menu entries to create across main desktop systems.
    pub menu_items: Vec<MenuItem>,
}

#[cfg(test)]
mod test {
    use crate::render::BaseMenuItemPlaceholders;
    use rattler_conda_types::Platform;
    use std::path::{Path, PathBuf};

    pub(crate) fn test_data() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("test-data")
    }

    #[test]
    fn test_deserialize_gnuradio() {
        let test_data = test_data();
        let schema_path = test_data.join("gnuradio/gnuradio-grc.json");
        let schema_str = std::fs::read_to_string(schema_path).unwrap();
        let schema: super::MenuInstSchema = serde_json::from_str(&schema_str).unwrap();
        insta::assert_debug_snapshot!(schema);
    }

    #[test]
    fn test_deserialize_mne() {
        let test_data = test_data();
        let schema_path = test_data.join("mne/menu.json");
        let schema_str = std::fs::read_to_string(schema_path).unwrap();
        let schema: super::MenuInstSchema = serde_json::from_str(&schema_str).unwrap();
        insta::assert_debug_snapshot!(schema);
    }

    #[test]
    fn test_deserialize_grx() {
        let test_data = test_data();
        let schema_path = test_data.join("gqrx/gqrx-menu.json");
        let schema_str = std::fs::read_to_string(schema_path).unwrap();
        let schema: super::MenuInstSchema = serde_json::from_str(&schema_str).unwrap();
        insta::assert_debug_snapshot!(schema);
    }

    #[test]
    fn test_deserialize_spyder() {
        let test_data = test_data();
        let schema_path = test_data.join("spyder/menu.json");
        let schema_str = std::fs::read_to_string(schema_path).unwrap();
        let schema: super::MenuInstSchema = serde_json::from_str(&schema_str).unwrap();

        let item = schema.menu_items[0].clone();
        let macos_item = item.platforms.osx.clone().unwrap();
        let command = item.command.merge(macos_item.base);
        let placeholders = BaseMenuItemPlaceholders::new(
            Path::new("base_prefix"),
            Path::new("prefix"),
            Platform::Linux64,
        );

        assert_eq!(
            command
                .name
                .resolve(super::Environment::Base, &placeholders),
            "Spyder 6 (prefix)"
        );

        insta::assert_debug_snapshot!(schema);
    }

    /// Test against the defaults file from original menuinst
    #[test]
    fn test_deserialize_defaults() {
        let test_data = test_data();
        let schema_path = test_data.join("defaults/defaults.json");
        let schema_str = std::fs::read_to_string(schema_path).unwrap();
        let schema: super::MenuInstSchema = serde_json::from_str(&schema_str).unwrap();
        insta::assert_debug_snapshot!(schema);
    }
}
