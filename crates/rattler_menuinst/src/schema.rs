use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::render::{BaseMenuItemPlaceholders, PlaceholderString};

#[derive(Serialize, Deserialize, Debug)]
#[serde(deny_unknown_fields)]
pub struct MenuItemNameDict {
    target_environment_is_base: Option<String>,
    target_environment_is_not_base: Option<String>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(deny_unknown_fields)]
pub struct BasePlatformSpecific {
    pub name: Option<NameField>,
    pub description: Option<PlaceholderString>,
    pub icon: Option<PlaceholderString>,
    pub command: Option<Vec<PlaceholderString>>,
    pub working_dir: Option<PlaceholderString>,
    pub precommand: Option<PlaceholderString>,
    pub precreate: Option<PlaceholderString>,
    pub activate: Option<bool>,
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
    NotBase,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(deny_unknown_fields)]
pub struct Windows {
    desktop: Option<bool>,
    quicklaunch: Option<bool>,
    terminal_profile: Option<String>,
    url_protocols: Option<Vec<String>>,
    file_extensions: Option<Vec<String>>,
    app_user_model_id: Option<String>,
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

/// Describes a URL scheme associated with the app.
#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(deny_unknown_fields)]
pub struct CFBundleURLTypesModel {
    /// This key specifies the app's role with respect to the URL.
    /// Can be one of `Editor`, `Viewer`, `Shell`, `None`
    #[serde(rename = "CFBundleTypeRole")]
    pub cf_bundle_type_role: Option<String>,

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
    pub cf_bundle_type_role: Option<String>,

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
    /// Can be one of `Owner`, `Default` or `Alternate`
    #[serde(rename = "LSHandlerRank")]
    pub ls_handler_rank: String, // TODO implement validation
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
    /// For example, "my app one two three" instead of "MyApp123".
    #[serde(rename = "CFBundleSpokenName")]
    pub cf_bundle_spoken_name: Option<String>,

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
    pub ls_environment: Option<HashMap<String, String>>,

    /// Minimum version of macOS required for this app to run, as `x.y.z`.
    /// For example, for macOS v10.4 and later, use `10.4.0`. (TODO: implement proper parsing)
    #[serde(rename = "LSMinimumSystemVersion")]
    pub ls_minimum_system_version: Option<String>,

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

    /// List of permissions to request for the launched application.
    /// See `the entitlements docs <https://developer.apple.com/documentation/bundleresources/entitlements>`
    /// for a full list of possible values.
    pub entitlements: Option<Vec<String>>,

    /// Paths that should be symlinked into the shortcut app bundle.
    /// It takes a mapping of source to destination paths. Destination paths must be
    /// relative. Placeholder `{{ MENU_ITEM_LOCATION }}` can be useful.
    pub link_in_bundle: Option<HashMap<PlaceholderString, PlaceholderString>>,

    /// Required shell script logic to handle opened URL payloads.
    pub event_handler: Option<String>,
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

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(deny_unknown_fields)]
pub struct MenuItemCommand {
    pub name: NameField,
    pub description: PlaceholderString,
    pub command: Vec<PlaceholderString>,
    pub icon: Option<PlaceholderString>,
    pub precommand: Option<PlaceholderString>,
    pub precreate: Option<PlaceholderString>,
    pub working_dir: Option<PlaceholderString>,
    pub activate: Option<bool>,
    pub terminal: Option<bool>,
}

impl MenuItemCommand {
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

#[derive(Serialize, Deserialize, Debug)]
#[serde(deny_unknown_fields)]
pub struct MenuInstSchema {
    #[serde(rename = "$id")]
    pub id: String,
    #[serde(rename = "$schema")]
    pub schema: String,
    pub menu_name: String,
    pub menu_items: Vec<MenuItem>,
}

#[cfg(test)]
mod test {
    use crate::render::BaseMenuItemPlaceholders;
    use rattler_conda_types::Platform;
    use std::path::{Path, PathBuf};

    pub(crate) fn test_data() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../test-data/menuinst")
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
            "Spyder 6 ({{ DISTRIBUTION_NAME }})"
        );

        // let foo = menu_0.platforms.osx.as_ref().unwrap().base.
        // get_name(super::Environment::Base);
        insta::assert_debug_snapshot!(schema);
    }
}