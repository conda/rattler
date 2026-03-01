mod build_number_spec;
mod error;
mod gateway;
mod match_spec;
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
