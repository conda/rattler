//! Parsers for the metadata files in the `info/` directory of a conda
//! package.

use rattler_conda_types::package::{AboutJson, IndexJson, PathsJson, RunExportsJson};
use serde::{Serialize, de::DeserializeOwned};
use wasm_bindgen::prelude::*;

use crate::JsResult;

#[wasm_bindgen(typescript_custom_section)]
const PACKAGE_METADATA_D_TS: &'static str = include_str!("package_metadata.d.ts");

#[wasm_bindgen]
extern "C" {
    #[wasm_bindgen(typescript_type = "IndexJson")]
    pub type JsIndexJson;

    #[wasm_bindgen(typescript_type = "AboutJson")]
    pub type JsAboutJson;

    #[wasm_bindgen(typescript_type = "PathsJson")]
    pub type JsPathsJson;

    #[wasm_bindgen(typescript_type = "RunExportsJson")]
    pub type JsRunExportsJson;
}

/// Parses `input`, the contents of a metadata file or an already parsed JSON
/// value, and returns its canonical JSON representation.
fn parse<T: DeserializeOwned + Serialize>(input: JsValue) -> JsResult<JsValue> {
    let value: T = match input.as_string() {
        Some(contents) => serde_json::from_str(&contents)?,
        None => serde_wasm_bindgen::from_value(input)?,
    };
    let serializer = serde_wasm_bindgen::Serializer::json_compatible();
    Ok(value.serialize(&serializer)?)
}

/// Parses and validates the contents of an `info/index.json` file.
///
/// @public
#[wasm_bindgen(js_name = "parseIndexJson")]
pub fn parse_index_json(
    #[wasm_bindgen(
        param_description = "The contents of the file, or its already parsed JSON value",
        unchecked_param_type = "string | object"
    )]
    input: JsValue,
) -> JsResult<JsIndexJson> {
    Ok(parse::<IndexJson>(input)?.into())
}

/// Parses and validates the contents of an `info/about.json` file.
///
/// @public
#[wasm_bindgen(js_name = "parseAboutJson")]
pub fn parse_about_json(
    #[wasm_bindgen(
        param_description = "The contents of the file, or its already parsed JSON value",
        unchecked_param_type = "string | object"
    )]
    input: JsValue,
) -> JsResult<JsAboutJson> {
    Ok(parse::<AboutJson>(input)?.into())
}

/// Parses and validates the contents of an `info/paths.json` file.
///
/// @public
#[wasm_bindgen(js_name = "parsePathsJson")]
pub fn parse_paths_json(
    #[wasm_bindgen(
        param_description = "The contents of the file, or its already parsed JSON value",
        unchecked_param_type = "string | object"
    )]
    input: JsValue,
) -> JsResult<JsPathsJson> {
    Ok(parse::<PathsJson>(input)?.into())
}

/// Parses and validates the contents of an `info/run_exports.json` file.
///
/// @public
#[wasm_bindgen(js_name = "parseRunExportsJson")]
pub fn parse_run_exports_json(
    #[wasm_bindgen(
        param_description = "The contents of the file, or its already parsed JSON value",
        unchecked_param_type = "string | object"
    )]
    input: JsValue,
) -> JsResult<JsRunExportsJson> {
    Ok(parse::<RunExportsJson>(input)?.into())
}
