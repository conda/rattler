use crate::{project_root, reformat, update, Mode};
use tempdir::TempDir;

const ALLOWED_FUNC_PREFIX: &[&str] = &[
    "map",
    "policy",
    "pool",
    "prune",
    "queue",
    "repo",
    "repodata",
    "selection",
    "solv",
    "solver",
    "testcase",
    "transaction",
    "dataiterator",
    "datamatcher",
    "stringpool",
];

const ALLOWED_VAR_PREFIX: &[&str] = &[
    "SOLVER_",
    "DISTTYPE_",
    "REL_",
    "CONDA_",
    "SELECTION_",
    "SEARCH_",
    "POOL_",
    "SOLV_",
];

/// Generate or verify the libsolv bindings.
pub fn generate(mode: Mode) -> anyhow::Result<()> {
    let libsolv_path = project_root().join("crates/rattler_solve/libsolv");

    // The bindings for libsolv are different for Unix and Windows because of the use of lib types.
    // To work around that issue we generate bindings for the two different platforms seperately.
    let suffix = if cfg!(windows) {
        "windows"
    } else if cfg!(unix) {
        "unix"
    } else {
        anyhow::bail!("only unix and windows are supported platforms for libsolv bindings");
    };

    // Normally the `solvversion.h` is generated from the `solverversion.h.in` by CMake when
    // building libsolv. However, for the bindings we don't need that much information from that
    // file. So to be able to generate proper bindings we use a drop-in-replacement for this file
    // that contains just the macros that are needed for the bindings.
    //
    // The behavior of this file might change when we update libsolv so its important to check this
    // with every upgrade.
    let temp_include_dir = TempDir::new("libsolv")?;
    std::fs::write(
        temp_include_dir.path().join("solvversion.h"),
        r#"#ifndef LIBSOLV_SOLVVERSION_H
#define LIBSOLV_SOLVVERSION_H

#define SOLV_API

#define LIBSOLV_FEATURE_MULTI_SEMANTICS
#define LIBSOLV_FEATURE_CONDA

#define LIBSOLVEXT_FEATURE_ZLIB_COMPRESSION

#endif
"#,
    )?;

    // Define the contents of the bindings and how they are generated
    let bindings = bindgen::Builder::default()
        .clang_arg(format!("-I{}", temp_include_dir.path().display()))
        .ctypes_prefix("libc")
        .header(libsolv_path.join("src/solver.h").to_str().unwrap())
        .header(libsolv_path.join("src/solverdebug.h").to_str().unwrap())
        .header(libsolv_path.join("src/queue.h").to_str().unwrap())
        .header(libsolv_path.join("src/pool.h").to_str().unwrap())
        .header(libsolv_path.join("src/selection.h").to_str().unwrap())
        .header(libsolv_path.join("src/knownid.h").to_str().unwrap())
        .header(libsolv_path.join("src/conda.h").to_str().unwrap())
        .header(libsolv_path.join("src/repo_solv.h").to_str().unwrap())
        .header(libsolv_path.join("src/repo_write.h").to_str().unwrap())
        .header(libsolv_path.join("ext/repo_conda.h").to_str().unwrap())
        .allowlist_type("(Id|solv_knownid)")
        .allowlist_var(format!("({}).*", ALLOWED_VAR_PREFIX.join("|")))
        .allowlist_function(format!("({}).*", ALLOWED_FUNC_PREFIX.join("|")))
        .blocklist_type("FILE")
        .disable_header_comment()
        .generate()?;

    // Generate the actual bindings and format them
    let mut libsolv_bindings = Vec::new();
    bindings.write(Box::new(&mut libsolv_bindings))?;

    // Add a preemble to the bindings to ensure clippy also passes.
    let libsolv_bindings = reformat(format!("#![allow(non_upper_case_globals, non_camel_case_types, non_snake_case, dead_code, clippy::upper_case_acronyms)]\n\npub use libc::FILE;\n\n{}", String::from_utf8(libsolv_bindings).unwrap()))?;

    // Write (or check) the bindings
    update(
        &project_root().join(format!(
            "crates/rattler_solve/src/libsolv/wrapper/ffi.rs"
        )),
        &libsolv_bindings,
        mode,
    )
}
