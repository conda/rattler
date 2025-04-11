use crate::{project_root, reformat, update, Mode};
use std::borrow::Cow;
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

const DISALLOWED_TYPES: &[&str] = &[
    // Dont generate code for libc FILE io related types. We add the binding to libc::FILE manually.
    // All the other types here are generated recursively through `FILE` for different platforms. We
    // also get rid of those because they are never used.
    "FILE", "_iobuf", "fpos_t", "_IO_.*",
    // Types starting with `__` are usually part of libc. We dont want them.
    "__.*",
];

/// Different platforms generate different bindings for enum representations. To not have to
/// generate completely different files based on the platform we patch these enum representations
/// inline.
fn patch_enum_representation<'a>(input: &'a str, enum_name: &str) -> Cow<'a, str> {
    let replacement = format!(
        concat!(
            "\ncfg_if::cfg_if! {{\n",
            "    if #[cfg(all(target_os = \"windows\", target_env = \"msvc\"))] {{\n",
            "        pub type {} = libc::c_int;\n",
            "    }} else {{\n",
            "        pub type {} = libc::c_uint;\n",
            "    }}\n",
            "}}"
        ),
        enum_name, enum_name
    );

    let c_int_repr = format!("\npub type {enum_name} = libc::c_int;");
    let c_uint_repr = format!("\npub type {enum_name} = libc::c_uint;");

    if let Some((start, str)) = input
        .match_indices(&c_int_repr)
        .next()
        .or_else(|| input.match_indices(&c_uint_repr).next())
    {
        Cow::Owned(format!(
            "{}{replacement}{}",
            &input[..start],
            &input[start + str.len()..]
        ))
    } else {
        Cow::Borrowed(input)
    }
}

/// Generate or verify the libsolv bindings.
pub fn generate(mode: Mode) -> anyhow::Result<()> {
    let libsolv_path = project_root().join("crates/rattler_libsolv_c/libsolv");

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
        .rust_target("1.85.0".parse().unwrap())
        .clang_arg(format!("-I{}", temp_include_dir.path().display()))
        .clang_arg(format!("-I{}", libsolv_path.join("src").display()))
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
        .allowlist_type("Id")
        .allowlist_var(format!("({}).*", ALLOWED_VAR_PREFIX.join("|")))
        .allowlist_function(format!("({}).*", ALLOWED_FUNC_PREFIX.join("|")))
        .blocklist_type(DISALLOWED_TYPES.join("|"))
        .disable_header_comment()
        .layout_tests(false)
        .generate()?;

    // Generate the actual bindings and format them
    let mut libsolv_bindings = Vec::new();
    bindings.write(Box::new(&mut libsolv_bindings))?;

    // Add a preemble to the bindings to ensure clippy also passes.
    let libsolv_bindings = reformat(format!("#![allow(non_upper_case_globals, non_camel_case_types, non_snake_case, dead_code, clippy::upper_case_acronyms)]\n\npub use libc::FILE;\n\n{}", String::from_utf8(libsolv_bindings).unwrap()))?;

    // Patch out some platform weirdness
    let libsolv_bindings = patch_enum_representation(&libsolv_bindings, "SolverRuleinfo");

    // Write (or check) the bindings
    update(
        &project_root().join("crates/rattler_libsolv_c/src/lib.rs"),
        &libsolv_bindings,
        mode,
    )
}
