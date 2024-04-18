use crate::install::PythonInfo;
use digest::Output;
use rattler_conda_types::{
    package::EntryPoint,
    prefix_record::{PathType, PathsEntry},
    Platform,
};
use rattler_digest::HashingWriter;
use rattler_digest::Sha256;
use std::{
    fs::{self, File},
    io::{self, Cursor, Write},
    path::{Path, PathBuf},
};
use zip::{write::FileOptions, ZipWriter};

/// Get the bytes of the windows launcher executable.
pub fn get_windows_launcher(platform: &Platform) -> &'static [u8] {
    match platform {
        Platform::Win32 => unimplemented!("32 bit windows is not supported for entry points"),
        Platform::Win64 => include_bytes!("../../resources/uv-trampoline-x86_64-console.exe"),
        Platform::WinArm64 => include_bytes!("../../resources/uv-trampoline-aarch64-console.exe"),
        _ => panic!("unsupported platform"),
    }
}

const LAUNCHER_MAGIC_NUMBER: [u8; 4] = [b'U', b'V', b'U', b'V'];

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
) -> Result<[PathsEntry; 1], std::io::Error> {
    let script_contents = python_entry_point_template(target_prefix, entry_point, python_info);

    // Construct a path to where we will create the python launcher executable.
    let relative_path_script_exe = python_info
        .bin_dir
        .join(format!("{}.exe", &entry_point.command));

    // Include the bytes of the launcher directly in the binary so we can write it to disk.
    let launcher_bytes = get_windows_launcher(target_platform);

    let mut payload: Vec<u8> = Vec::new();
    {
        // We're using the zip writer, but with stored compression
        // https://github.com/njsmith/posy/blob/04927e657ca97a5e35bb2252d168125de9a3a025/src/trampolines/mod.rs#L75-L82
        // https://github.com/pypa/distlib/blob/8ed03aab48add854f377ce392efffb79bb4d6091/PC/launcher.c#L259-L271
        let stored = FileOptions::default().compression_method(zip::CompressionMethod::Stored);
        let mut archive = ZipWriter::new(Cursor::new(&mut payload));
        let error_msg = "Writing to Vec<u8> should never fail";
        archive.start_file("__main__.py", stored).expect(error_msg);
        archive
            .write_all(script_contents.as_bytes())
            .expect(error_msg);
        archive.finish().expect(error_msg);
    }

    let python = PathBuf::from(target_prefix).join(&python_info.path);
    let python_path = dunce::simplified(&python).display().to_string();
    println!("Python path: {}", python_path);

    let mut launcher: Vec<u8> = Vec::with_capacity(launcher_bytes.len() + payload.len());
    launcher.extend_from_slice(launcher_bytes);
    launcher.extend_from_slice(&payload);
    launcher.extend_from_slice(python_path.as_bytes());
    launcher.extend_from_slice(
        &u32::try_from(python_path.as_bytes().len())
            .expect("File Path to be smaller than 4GB")
            .to_le_bytes(),
    );
    launcher.extend_from_slice(&LAUNCHER_MAGIC_NUMBER);

    let target_location = target_dir.join(&relative_path_script_exe);
    fs::create_dir_all(target_location.parent().unwrap())?;
    let (sha256, size) = write_and_hash(&target_location, launcher)?;

    Ok([PathsEntry {
        relative_path: relative_path_script_exe,
        original_path: None,
        path_type: PathType::WindowsPythonEntryPointExe,
        no_link: false,
        sha256: Some(sha256),
        sha256_in_prefix: None,
        size_in_bytes: Some(size as u64),
    }])
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
    let script_contents = python_entry_point_template(target_prefix, entry_point, python_info);
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
    })
}

/// Returns Python code that, when placed in an executable file, invokes the specified
/// [`EntryPoint`].
pub fn python_entry_point_template(
    target_prefix: &str,
    entry_point: &EntryPoint,
    python_info: &PythonInfo,
) -> String {
    // Construct a shebang for the python interpreter
    let shebang = python_info.shebang(target_prefix);

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
            &EntryPoint::from_str("jupyter-lab = jupyterlab.labapp:main").unwrap(),
            &PythonInfo::from_version(&Version::from_str("3.11.0").unwrap(), Platform::Linux64)
                .unwrap(),
        );
        insta::assert_snapshot!(script);
    }
}
