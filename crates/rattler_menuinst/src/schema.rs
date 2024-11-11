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

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(deny_unknown_fields)]
pub struct Linux {
    #[serde(rename = "Categories")]
    pub categories: Option<Vec<PlaceholderString>>,
    #[serde(rename = "DBusActivatable")]
    pub dbus_activatable: Option<bool>,
    #[serde(rename = "GenericName")]
    pub generic_name: Option<PlaceholderString>,
    #[serde(rename = "Hidden")]
    pub hidden: Option<bool>,
    #[serde(rename = "Implements")]
    pub implements: Option<Vec<PlaceholderString>>,
    #[serde(rename = "Keywords")]
    pub keywords: Option<Vec<PlaceholderString>>,
    #[serde(rename = "MimeType")]
    pub mime_type: Option<Vec<PlaceholderString>>,
    #[serde(rename = "NoDisplay")]
    pub no_display: Option<bool>,
    #[serde(rename = "NotShowIn")]
    pub not_show_in: Option<Vec<PlaceholderString>>,
    #[serde(rename = "OnlyShowIn")]
    pub only_show_in: Option<Vec<PlaceholderString>>,
    #[serde(rename = "PrefersNonDefaultGPU")]
    pub prefers_non_default_gpu: Option<bool>,
    #[serde(rename = "StartupNotify")]
    pub startup_notify: Option<bool>,
    #[serde(rename = "StartupWMClass")]
    pub startup_wm_class: Option<PlaceholderString>,
    #[serde(rename = "TryExec")]
    pub try_exec: Option<PlaceholderString>,
    pub glob_patterns: Option<HashMap<PlaceholderString, PlaceholderString>>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(deny_unknown_fields)]
pub struct CFBundleURLTypesModel {
    #[serde(rename = "CFBundleTypeRole")]
    cf_bundle_type_role: Option<PlaceholderString>,
    #[serde(rename = "CFBundleURLSchemes")]
    cf_bundle_url_schemes: Vec<PlaceholderString>,
    #[serde(rename = "CFBundleURLName")]
    cf_bundle_url_name: Option<PlaceholderString>,
    #[serde(rename = "CFBundleURLIconFile")]
    cf_bundle_url_icon_file: Option<PlaceholderString>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(deny_unknown_fields)]
pub struct CFBundleDocumentTypesModel {
    #[serde(rename = "CFBundleTypeIconFile")]
    cf_bundle_type_icon_file: Option<PlaceholderString>,
    #[serde(rename = "CFBundleTypeName")]
    cf_bundle_type_name: PlaceholderString,
    #[serde(rename = "CFBundleTypeRole")]
    cf_bundle_type_role: Option<PlaceholderString>,
    #[serde(rename = "LSItemContentTypes")]
    ls_item_content_types: Vec<PlaceholderString>,
    #[serde(rename = "LSHandlerRank")]
    ls_handler_rank: PlaceholderString,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(deny_unknown_fields)]
pub struct UTTypeDeclarationModel {
    #[serde(rename = "UTTypeConformsTo")]
    ut_type_conforms_to: Vec<PlaceholderString>,
    #[serde(rename = "UTTypeDescription")]
    ut_type_description: Option<PlaceholderString>,
    #[serde(rename = "UTTypeIconFile")]
    ut_type_icon_file: Option<PlaceholderString>,
    #[serde(rename = "UTTypeIdentifier")]
    ut_type_identifier: PlaceholderString,
    #[serde(rename = "UTTypeReferenceURL")]
    ut_type_reference_url: Option<PlaceholderString>,
    #[serde(rename = "UTTypeTagSpecification")]
    ut_type_tag_specification: HashMap<PlaceholderString, Vec<PlaceholderString>>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(deny_unknown_fields)]
pub struct MacOS {
    #[serde(rename = "CFBundleDisplayName")]
    pub cf_bundle_display_name: Option<PlaceholderString>,
    #[serde(rename = "CFBundleIdentifier")]
    pub cf_bundle_identifier: Option<PlaceholderString>,
    #[serde(rename = "CFBundleName")]
    pub cf_bundle_name: Option<PlaceholderString>,
    #[serde(rename = "CFBundleSpokenName")]
    pub cf_bundle_spoken_name: Option<String>,
    #[serde(rename = "CFBundleVersion")]
    pub cf_bundle_version: Option<String>,
    #[serde(rename = "CFBundleURLTypes")]
    pub cf_bundle_url_types: Option<Vec<CFBundleURLTypesModel>>,
    #[serde(rename = "CFBundleDocumentTypes")]
    pub cf_bundle_document_types: Option<Vec<CFBundleDocumentTypesModel>>,
    #[serde(rename = "LSApplicationCategoryType")]
    pub ls_application_category_type: Option<String>,
    #[serde(rename = "LSBackgroundOnly")]
    pub ls_background_only: Option<bool>,
    #[serde(rename = "LSEnvironment")]
    pub ls_environment: Option<HashMap<String, String>>,
    #[serde(rename = "LSMinimumSystemVersion")]
    pub ls_minimum_system_version: Option<String>,
    #[serde(rename = "LSMultipleInstancesProhibited")]
    pub ls_multiple_instances_prohibited: Option<bool>,
    #[serde(rename = "LSRequiresNativeExecution")]
    pub ls_requires_native_execution: Option<bool>,
    #[serde(rename = "NSSupportsAutomaticGraphicsSwitching")]
    pub ns_supports_automatic_graphics_switching: Option<bool>,
    #[serde(rename = "UTExportedTypeDeclarations")]
    pub ut_exported_type_declarations: Option<Vec<UTTypeDeclarationModel>>,
    #[serde(rename = "UTImportedTypeDeclarations")]
    pub ut_imported_type_declarations: Option<Vec<UTTypeDeclarationModel>>,
    pub entitlements: Option<Vec<String>>,
    pub link_in_bundle: Option<HashMap<PlaceholderString, PlaceholderString>>,
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
