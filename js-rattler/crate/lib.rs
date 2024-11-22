mod utils;
mod version;
mod version_spec;
mod error;

use rattler_conda_types::ParseStrictness;
pub use error::{JsError, JsResult};

use wasm_bindgen::prelude::*;

// When the `wee_alloc` feature is enabled, use `wee_alloc` as the global
// allocator.
#[cfg(feature = "wee_alloc")]
#[global_allocator]
static ALLOC: wee_alloc::WeeAlloc = wee_alloc::WeeAlloc::INIT;

/// This function is called when the wasm module is instantiated.
#[wasm_bindgen(start)]
pub fn start() {
    utils::set_panic_hook();
}

#[wasm_bindgen(js_name=ParseStrictness)]
pub enum JsParseStrictness {
    Strict,
    Lenient,
}

impl From<ParseStrictness> for JsParseStrictness {
    fn from(value: ParseStrictness) -> Self {
        match value {
            ParseStrictness::Strict => JsParseStrictness::Strict,
            ParseStrictness::Lenient => JsParseStrictness::Lenient,
        }
    }
}

impl From<JsParseStrictness> for ParseStrictness {
    fn from(value: JsParseStrictness) -> Self {
        match value {
            JsParseStrictness::Strict => ParseStrictness::Strict,
            JsParseStrictness::Lenient => ParseStrictness::Lenient,
        }
    }
}