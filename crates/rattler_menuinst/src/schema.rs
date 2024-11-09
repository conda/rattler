use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Serialize, Deserialize, Debug)]
#[serde(deny_unknown_fields)]
pub struct MenuItemNameDict {
    target_environment_is_base: Option<String>,
    target_environment_is_not_base: Option<String>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(deny_unknown_fields)]
pub struct BasePlatformSpecific {
    #[serde(default)]
    pub name: Option<NameField>,
    #[serde(default)]
    pub description: String,
    pub icon: Option<String>,
    #[serde(default)]
    pub command: Vec<String>,
    pub working_dir: Option<String>,
    pub precommand: Option<String>,
    pub precreate: Option<String>,
    pub activate: Option<bool>,
    pub terminal: Option<bool>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(untagged)]
pub enum NameField {
    Simple(String),
    Complex(NameComplex),
}

impl BasePlatformSpecific {
    pub fn get_name(&self, env: Environment) -> &str {
        match self.name.as_ref().unwrap() {
            NameField::Simple(name) => name,
            NameField::Complex(complex_name) => match env {
                Environment::Base => &complex_name.target_environment_is_base,
                Environment::NotBase => &complex_name.target_environment_is_not_base,
            },
        }
    }
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct NameComplex {
    pub target_environment_is_base: String,
    pub target_environment_is_not_base: String,
}

pub enum Environment {
    Base,
    NotBase,
}

impl BasePlatformSpecific {
    pub fn merge_parent(self, parent: &MenuItem) -> Self {
        let name = if self.name.is_none() {
            parent.name.clone()
        } else {
            self.name.unwrap()
        };

        let description = if self.description.is_empty() {
            parent.description.clone()
        } else {
            self.description
        };

        let command = if self.command.is_empty() {
            parent.command.clone()
        } else {
            self.command
        };

        BasePlatformSpecific {
            name: Some(name),
            description,
            icon: self.icon.or(parent.icon.clone()),
            command,
            working_dir: self.working_dir.or(parent.working_dir.clone()),
            precommand: self.precommand.or(parent.precommand.clone()),
            precreate: self.precreate.or(parent.precreate.clone()),
            activate: self.activate.or(parent.activate),
            terminal: self.terminal.or(parent.terminal),
        }
    }
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(deny_unknown_fields)]
pub struct Windows {
    #[serde(flatten)]
    base: BasePlatformSpecific,
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
    #[serde(flatten)]
    pub base: BasePlatformSpecific,
    #[serde(rename = "Categories")]
    pub categories: Option<Vec<String>>,
    #[serde(rename = "DBusActivatable")]
    pub dbus_activatable: Option<bool>,
    #[serde(rename = "GenericName")]
    pub generic_name: Option<String>,
    #[serde(rename = "Hidden")]
    pub hidden: Option<bool>,
    #[serde(rename = "Implements")]
    pub implements: Option<Vec<String>>,
    #[serde(rename = "Keywords")]
    pub keywords: Option<Vec<String>>,
    #[serde(rename = "MimeType")]
    pub mime_type: Option<Vec<String>>,
    #[serde(rename = "NoDisplay")]
    pub no_display: Option<bool>,
    #[serde(rename = "NotShowIn")]
    pub not_show_in: Option<Vec<String>>,
    #[serde(rename = "OnlyShowIn")]
    pub only_show_in: Option<Vec<String>>,
    #[serde(rename = "PrefersNonDefaultGPU")]
    pub prefers_non_default_gpu: Option<bool>,
    #[serde(rename = "StartupNotify")]
    pub startup_notify: Option<bool>,
    #[serde(rename = "StartupWMClass")]
    pub startup_wm_class: Option<String>,
    #[serde(rename = "TryExec")]
    pub try_exec: Option<String>,
    pub glob_patterns: Option<HashMap<String, String>>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(deny_unknown_fields)]
pub struct CFBundleURLTypesModel {
    #[serde(rename = "CFBundleTypeRole")]
    cf_bundle_type_role: Option<String>,
    #[serde(rename = "CFBundleURLSchemes")]
    cf_bundle_url_schemes: Vec<String>,
    #[serde(rename = "CFBundleURLName")]
    cf_bundle_url_name: Option<String>,
    #[serde(rename = "CFBundleURLIconFile")]
    cf_bundle_url_icon_file: Option<String>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(deny_unknown_fields)]
pub struct CFBundleDocumentTypesModel {
    #[serde(rename = "CFBundleTypeIconFile")]
    cf_bundle_type_icon_file: Option<String>,
    #[serde(rename = "CFBundleTypeName")]
    cf_bundle_type_name: String,
    #[serde(rename = "CFBundleTypeRole")]
    cf_bundle_type_role: Option<String>,
    #[serde(rename = "LSItemContentTypes")]
    ls_item_content_types: Vec<String>,
    #[serde(rename = "LSHandlerRank")]
    ls_handler_rank: String,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(deny_unknown_fields)]
pub struct UTTypeDeclarationModel {
    #[serde(rename = "UTTypeConformsTo")]
    ut_type_conforms_to: Vec<String>,
    #[serde(rename = "UTTypeDescription")]
    ut_type_description: Option<String>,
    #[serde(rename = "UTTypeIconFile")]
    ut_type_icon_file: Option<String>,
    #[serde(rename = "UTTypeIdentifier")]
    ut_type_identifier: String,
    #[serde(rename = "UTTypeReferenceURL")]
    ut_type_reference_url: Option<String>,
    #[serde(rename = "UTTypeTagSpecification")]
    ut_type_tag_specification: HashMap<String, Vec<String>>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(deny_unknown_fields)]
pub struct MacOS {
    #[serde(flatten)]
    pub base: BasePlatformSpecific,
    #[serde(rename = "CFBundleDisplayName")]
    pub cf_bundle_display_name: Option<String>,
    #[serde(rename = "CFBundleIdentifier")]
    pub cf_bundle_identifier: Option<String>,
    #[serde(rename = "CFBundleName")]
    pub cf_bundle_name: Option<String>,
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
    pub link_in_bundle: Option<HashMap<String, String>>,
    pub event_handler: Option<String>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(deny_unknown_fields)]
pub struct Platforms {
    pub linux: Option<Linux>,
    pub osx: Option<MacOS>,
    pub win: Option<Windows>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(deny_unknown_fields)]
pub struct MenuItem {
    pub name: NameField,
    pub description: String,
    pub command: Vec<String>,
    pub icon: Option<String>,
    pub precommand: Option<String>,
    pub precreate: Option<String>,
    pub working_dir: Option<String>,
    pub activate: Option<bool>,
    pub terminal: Option<bool>,
    pub platforms: Platforms,
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
    use std::path::{Path, PathBuf};

    use crate::macos::Directories;

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
        let mut macos_item = item.platforms.osx.clone().unwrap();
        let base_item = macos_item.base.merge_parent(&item);
        macos_item.base = base_item;

        assert_eq!(
            macos_item.base.get_name(super::Environment::Base),
            "superspyder 1.2 (base)"
        );

        // let foo = menu_0.platforms.osx.as_ref().unwrap().base.get_name(super::Environment::Base);
        insta::assert_debug_snapshot!(schema);
    }
}
