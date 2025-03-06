use wasm_bindgen_test::*;

#[wasm_bindgen_test]
async fn pass() {
    js_rattler::solve::simple_solve(
        ["python"].into_iter().map(str::to_string).collect(),
        [
            "https://repo.prefix.dev/emscripten-forge-dev",
            "https://repo.prefix.dev/conda-forge",
        ]
        .into_iter()
        .map(str::to_string)
        .collect(),
        ["emscripten-wasm32", "noarch"]
            .into_iter()
            .map(str::to_string)
            .collect(),
    )
    .await
    .unwrap();
}

wasm_bindgen_test::wasm_bindgen_test_configure!(run_in_browser);
