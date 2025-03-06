mod error;
mod gateway;
mod parse_strictness;
pub mod solve;
mod utils;
mod version;
mod version_spec;

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
