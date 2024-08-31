use serde;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Serialize, Deserialize, Debug)]
#[serde(deny_unknown_fields)]
pub struct MenuItemNameDict {
    target_environment_is_base: Option<String>,
    target_environment_is_not_base: Option<String>,
}

#[derive(Serialize, Deserialize, Debug)]
#[serde(deny_unknown_fields)]
pub struct BasePlatformSpecific {
    name: Option<String>,
    description: Option<String>,
    icon: Option<String>,
    command: Option<Vec<String>>,
    working_dir: Option<String>,
    precommand: Option<String>,
    precreate: Option<String>,
    activate: Option<bool>,
    terminal: Option<bool>,
}

#[derive(Serialize, Deserialize, Debug)]
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

#[derive(Serialize, Deserialize, Debug)]
#[serde(deny_unknown_fields)]
pub struct Linux {
    #[serde(flatten)]
    base: BasePlatformSpecific,
    #[serde(rename = "Categories")]
    categories: Option<Vec<String>>,
    #[serde(rename = "DBusActivatable")]
    dbus_activatable: Option<bool>,
    #[serde(rename = "GenericName")]
    generic_name: Option<String>,
    #[serde(rename = "Hidden")]
    hidden: Option<bool>,
    #[serde(rename = "Implements")]
    implements: Option<Vec<String>>,
    #[serde(rename = "Keywords")]
    keywords: Option<Vec<String>>,
    #[serde(rename = "MimeType")]
    mime_type: Option<Vec<String>>,
    #[serde(rename = "NoDisplay")]
    no_display: Option<bool>,
    #[serde(rename = "NotShowIn")]
    not_show_in: Option<Vec<String>>,
    #[serde(rename = "OnlyShowIn")]
    only_show_in: Option<Vec<String>>,
    #[serde(rename = "PrefersNonDefaultGPU")]
    prefers_non_default_gpu: Option<bool>,
    #[serde(rename = "StartupNotify")]
    startup_notify: Option<bool>,
    #[serde(rename = "StartupWMClass")]
    startup_wm_class: Option<String>,
    #[serde(rename = "TryExec")]
    try_exec: Option<String>,
    glob_patterns: Option<HashMap<String, String>>,
}

#[derive(Serialize, Deserialize, Debug)]
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

#[derive(Serialize, Deserialize, Debug)]
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

#[derive(Serialize, Deserialize, Debug)]
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

#[derive(Serialize, Deserialize, Debug)]
#[serde(deny_unknown_fields)]
pub struct MacOS {
    #[serde(flatten)]
    base: BasePlatformSpecific,
    #[serde(rename = "CFBundleDisplayName")]
    cf_bundle_display_name: Option<String>,
    #[serde(rename = "CFBundleIdentifier")]
    cf_bundle_identifier: Option<String>,
    #[serde(rename = "CFBundleName")]
    cf_bundle_name: Option<String>,
    #[serde(rename = "CFBundleSpokenName")]
    cf_bundle_spoken_name: Option<String>,
    #[serde(rename = "CFBundleVersion")]
    cf_bundle_version: Option<String>,
    #[serde(rename = "CFBundleURLTypes")]
    cf_bundle_url_types: Option<Vec<CFBundleURLTypesModel>>,
    #[serde(rename = "CFBundleDocumentTypes")]
    cf_bundle_document_types: Option<Vec<CFBundleDocumentTypesModel>>,
    #[serde(rename = "LSApplicationCategoryType")]
    ls_application_category_type: Option<String>,
    #[serde(rename = "LSBackgroundOnly")]
    ls_background_only: Option<bool>,
    #[serde(rename = "LSEnvironment")]
    ls_environment: Option<HashMap<String, String>>,
    #[serde(rename = "LSMinimumSystemVersion")]
    ls_minimum_system_version: Option<String>,
    #[serde(rename = "LSMultipleInstancesProhibited")]
    ls_multiple_instances_prohibited: Option<bool>,
    #[serde(rename = "LSRequiresNativeExecution")]
    ls_requires_native_execution: Option<bool>,
    #[serde(rename = "NSSupportsAutomaticGraphicsSwitching")]
    ns_supports_automatic_graphics_switching: Option<bool>,
    #[serde(rename = "UTExportedTypeDeclarations")]
    ut_exported_type_declarations: Option<Vec<UTTypeDeclarationModel>>,
    #[serde(rename = "UTImportedTypeDeclarations")]
    ut_imported_type_declarations: Option<Vec<UTTypeDeclarationModel>>,
    entitlements: Option<Vec<String>>,
    link_in_bundle: Option<HashMap<String, String>>,
    event_handler: Option<String>,
}

#[derive(Serialize, Deserialize, Debug)]
#[serde(deny_unknown_fields)]
pub struct Platforms {
    linux: Option<Linux>,
    osx: Option<MacOS>,
    win: Option<Windows>,
}

#[derive(Serialize, Deserialize, Debug)]
#[serde(deny_unknown_fields)]
pub struct MenuItem {
    name: String,
    description: String,
    command: Vec<String>,
    icon: Option<String>,
    precommand: Option<String>,
    precreate: Option<String>,
    working_dir: Option<String>,
    activate: Option<bool>,
    terminal: Option<bool>,
    platforms: Platforms,
}

#[derive(Serialize, Deserialize, Debug)]
#[serde(deny_unknown_fields)]
pub struct MenuInstSchema {
    #[serde(rename = "$id")]
    id: String,
    #[serde(rename = "$schema")]
    schema: String,
    menu_name: String,
    menu_items: Vec<MenuItem>,
}

#[cfg(test)]
mod test {
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
}
