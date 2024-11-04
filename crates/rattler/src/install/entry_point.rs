use crate::install::PythonInfo;
use digest::Output;
use rattler_conda_types::{
    package::EntryPoint,
    prefix_record::{PathType, PathsEntry},
    Platform,
};
use rattler_digest::HashingWriter;
use rattler_digest::Sha256;
use std::{fs::File, io, io::Write, path::Path};

/// Get the bytes of the windows launcher executable.
pub fn get_windows_launcher(platform: &Platform) -> &'static [u8] {
    match platform {
        Platform::Win32 => unimplemented!("32 bit windows is not supported for entry points"),
        Platform::Win64 => include_bytes!("../../resources/launcher64.exe"),
        Platform::WinArm64 => unimplemented!("arm64 windows is not supported for entry points"),
        _ => panic!("unsupported platform"),
    }
}

/// Creates an "entry point" on disk for a Python entrypoint. Entrypoints are executable files that
/// directly call a certain Python function.
///
/// On unix this is pretty trivial through the use of an executable shell script that invokes the
/// python compiler which in turn invokes the correct Python function. On windows however, this
/// mechanism doesnt exists. Instead a special executable is copied that starts a Python interpreter
/// which executes a file that is named the same as the executable but with the `.py` file
/// extension. So if there is an entry point file called `foo.py` an executable is created called
/// `foo.exe` that will automatically invoke `foo.py`.
///
/// The special executable is embedded in the library. The source code for the launcher can be found
/// here: <https://github.com/conda/conda-build/tree/master/conda_build/launcher_sources>.
///
/// See [`create_unix_python_entry_point`] for the unix variant of this function.
pub fn create_windows_python_entry_point(
    target_dir: &Path,
    target_prefix: &str,
    entry_point: &EntryPoint,
    python_info: &PythonInfo,
    target_platform: &Platform,
) -> Result<[PathsEntry; 2], std::io::Error> {
    // Construct the path to where we will be creating the python entry point script.
    let relative_path_script_py = python_info
        .bin_dir
        .join(format!("{}-script.py", &entry_point.command));

    // Write the contents of the launcher script to disk
    let script_path = target_dir.join(&relative_path_script_py);
    std::fs::create_dir_all(
        script_path
            .parent()
            .expect("since we joined with target_dir there must be a parent"),
    )?;
    let script_contents =
        python_entry_point_template(target_prefix, true, entry_point, python_info);
    let (hash, size) = write_and_hash(&script_path, script_contents)?;

    // Construct a path to where we will create the python launcher executable.
    let relative_path_script_exe = python_info
        .bin_dir
        .join(format!("{}.exe", &entry_point.command));

    // Include the bytes of the launcher directly in the binary so we can write it to disk.
    let launcher_bytes = get_windows_launcher(target_platform);
    std::fs::write(target_dir.join(&relative_path_script_exe), launcher_bytes)?;

    let fixed_launcher_digest = rattler_digest::parse_digest_from_hex::<rattler_digest::Sha256>(
        "28b001bb9a72ae7a24242bfab248d767a1ac5dec981c672a3944f7a072375e9a",
    )
    .unwrap();

    Ok([
        PathsEntry {
            relative_path: relative_path_script_py,
            // todo: clobbering of entry points not handled yet
            original_path: None,
            path_type: PathType::WindowsPythonEntryPointScript,
            no_link: false,
            sha256: Some(hash),
            sha256_in_prefix: None,
            size_in_bytes: Some(size as _),
            prefix_placeholder: None,
            file_mode: None,
        },
        PathsEntry {
            relative_path: relative_path_script_exe,
            original_path: None,
            path_type: PathType::WindowsPythonEntryPointExe,
            no_link: false,
            sha256: Some(fixed_launcher_digest),
            sha256_in_prefix: None,
            size_in_bytes: Some(launcher_bytes.len() as u64),
            prefix_placeholder: None,
            file_mode: None,
        },
    ])
}

/// Creates an "entry point" on disk for a Python entrypoint. Entrypoints are executable files that
/// directly call a certain Python function.
///
/// On unix this is pretty trivial through the use of an executable shell script that invokes the
/// python compiler which in turn invokes the correct Python function.
///
/// On windows things are a bit more complicated. See [`create_windows_python_entry_point`].
pub fn create_unix_python_entry_point(
    target_dir: &Path,
    target_prefix: &str,
    entry_point: &EntryPoint,
    python_info: &PythonInfo,
) -> Result<PathsEntry, std::io::Error> {
    // Construct the path to where we will be creating the python entry point script.
    let relative_path = python_info.bin_dir.join(&entry_point.command);

    // Write the contents of the launcher script to disk
    let script_path = target_dir.join(&relative_path);
    std::fs::create_dir_all(
        script_path
            .parent()
            .expect("since we joined with target_dir there must be a parent"),
    )?;
    let script_contents =
        python_entry_point_template(target_prefix, false, entry_point, python_info);
    let (hash, size) = write_and_hash(&script_path, script_contents)?;

    // Make the script executable. This is only supported on Unix based filesystems.
    #[cfg(unix)]
    std::fs::set_permissions(
        script_path,
        std::os::unix::fs::PermissionsExt::from_mode(0o775),
    )?;

    Ok(PathsEntry {
        relative_path,
        // todo: clobbering of entry points not handled yet
        original_path: None,
        path_type: PathType::UnixPythonEntryPoint,
        no_link: false,
        sha256: Some(hash),
        sha256_in_prefix: None,
        size_in_bytes: Some(size as _),
        prefix_placeholder: None,
        file_mode: None,
    })
}

/// Returns Python code that, when placed in an executable file, invokes the specified
/// [`EntryPoint`].
pub fn python_entry_point_template(
    target_prefix: &str,
    for_windows: bool,
    entry_point: &EntryPoint,
    python_info: &PythonInfo,
) -> String {
    // Construct a shebang for the python interpreter
    let shebang = if for_windows {
        // On windows we don't need a shebang. Adding a shebang actually breaks the launcher
        // for prefixes with spaces.
        String::new()
    } else {
        python_info.shebang(target_prefix)
    };

    // The name of the module to import to be able to call the function
    let (import_name, _) = entry_point
        .function
        .split_once('.')
        .unwrap_or((&entry_point.function, ""));

    let module = &entry_point.module;
    let func = &entry_point.function;
    format!(
        "{shebang}\n\
        # -*- coding: utf-8 -*-\n\
        import re\n\
        import sys\n\n\
        from {module} import {import_name}\n\n\
        if __name__ == '__main__':\n\
        \tsys.argv[0] = re.sub(r'(-script\\.pyw?|\\.exe)?$', '', sys.argv[0])\n\
        \tsys.exit({func}())\n\
        "
    )
}

/// Writes the given bytes to a file and records the hash, as well as the size of the file.
fn write_and_hash(path: &Path, contents: impl AsRef<[u8]>) -> io::Result<(Output<Sha256>, usize)> {
    let bytes = contents.as_ref();
    let mut writer = HashingWriter::<_, Sha256>::new(File::create(path)?);
    writer.write_all(bytes)?;
    let (_, hash) = writer.finalize();
    Ok((hash, bytes.len()))
}

#[cfg(test)]
mod test {
    use crate::install::PythonInfo;
    use rattler_conda_types::package::EntryPoint;
    use rattler_conda_types::{Platform, Version};
    use std::str::FromStr;

    #[test]
    fn test_entry_point_script() {
        let script = super::python_entry_point_template(
            "/prefix",
            false,
            &EntryPoint::from_str("jupyter-lab = jupyterlab.labapp:main").unwrap(),
            &PythonInfo::from_version(
                &Version::from_str("3.11.0").unwrap(),
                None,
                Platform::Linux64,
            )
            .unwrap(),
        );
        insta::assert_snapshot!(script);

        let script = super::python_entry_point_template(
            "/prefix",
            true,
            &EntryPoint::from_str("jupyter-lab = jupyterlab.labapp:main").unwrap(),
            &PythonInfo::from_version(
                &Version::from_str("3.11.0").unwrap(),
                None,
                Platform::Linux64,
            )
            .unwrap(),
        );
        insta::assert_snapshot!("windows", script);
    }
}
