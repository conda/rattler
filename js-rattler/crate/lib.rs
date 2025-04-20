mod error;
mod gateway;
mod noarch_type;
mod package_name;
mod package_record;
mod parse_strictness;
mod platform;
pub mod solve;
mod utils;
mod version;
mod version_spec;
mod version_with_source;

pub use error::{JsError, JsResult};

use wasm_bindgen::prelude::*;
use rattler_networking::mirror_middleware::create_404_response;

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

#[wasm_bindgen]
pub fn create_wasm_404_response(url: &str, body: &str) -> Result<JsValue, JsError> {
    let url = url::Url::parse(url).map_err(|e| JsError::new(&e.to_string()))?;
    let response = create_404_response(&url, body);
    Ok(serde_wasm_bindgen::to_value(&response)?)
}
