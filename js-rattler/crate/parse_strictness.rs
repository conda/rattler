use rattler_conda_types::ParseStrictness;
use serde::Deserialize;
use wasm_bindgen::prelude::wasm_bindgen;

#[wasm_bindgen(typescript_custom_section)]
const PARSE_STRICTNESS_TS: &'static str = r#"
/**
 * Defines how strict a parser should be when parsing an object from a string.
 *
 * @public
 */
export type ParseStrictness = "strict" | "lenient";
"#;

#[wasm_bindgen]
extern "C" {
    #[wasm_bindgen(js_name = "ParseStrictness", typescript_type = "ParseStrictness")]
    pub type JsParseStrictness;
}

impl TryFrom<JsParseStrictness> for ParseStrictness {
    type Error = serde_wasm_bindgen::Error;

    fn try_from(value: JsParseStrictness) -> Result<Self, Self::Error> {
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase")]
        pub enum EnumNames {
            Strict,
            Lenient,
        }

        match serde_wasm_bindgen::from_value(value.obj)? {
            EnumNames::Strict => Ok(ParseStrictness::Strict),
            EnumNames::Lenient => Ok(ParseStrictness::Lenient),
        }
    }
}
