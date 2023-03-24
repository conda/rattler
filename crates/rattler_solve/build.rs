use anyhow::{anyhow, Result};
use std::path::{self, PathBuf};

fn build_libsolv() -> Result<PathBuf> {
    let target = std::env::var("TARGET").unwrap();
    let is_windows = target.contains("windows");

    let p = path::PathBuf::from("./libsolv/CMakeLists.txt");
    if !p.is_file() {
        return Err(anyhow!(
            "Bundled libsolv not found, please do `git submodule update --init`."
        ));
    }
    let out = cmake::Config::new(p.parent().unwrap())
        .define("ENABLE_CONDA", "ON")
        .define("ENABLE_STATIC", "ON")
        .define("DISABLE_SHARED", "ON")
        .define("MULTI_SEMANTICS", "ON")
        .define("WITHOUT_COOKIEOPEN", "ON")
        .register_dep("z")
        .target(&std::env::var("CMAKE_TARGET").unwrap_or_else(|_| std::env::var("TARGET").unwrap()))
        .build();
    println!(
        "cargo:rustc-link-search=native={}",
        out.join("lib").display()
    );
    println!(
        "cargo:rustc-link-search=native={}",
        out.join("lib64").display()
    );

    if is_windows {
        println!("cargo:rustc-link-lib=static=solv_static");
        println!("cargo:rustc-link-lib=static=solvext_static");
    } else {
        println!("cargo:rustc-link-lib=static=solv");
        println!("cargo:rustc-link-lib=static=solvext");
    }

    Ok(out.join("include/solv"))
}

fn main() -> Result<()> {
    build_libsolv()?;

    Ok(())
}
