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

/// This function is called when the wasm module is instantiated.
#[wasm_bindgen(start)]
pub fn start() {
    utils::set_panic_hook();
}
