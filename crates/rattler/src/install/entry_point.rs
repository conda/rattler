use crate::install::PythonInfo;
use digest::Output;
use rattler_conda_types::{
    Platform,
    package::EntryPoint,
    prefix_record::{PathType, PathsEntry},
};
use rattler_digest::HashingWriter;
use rattler_digest::Sha256;
use std::path::{Component, PathBuf};
use std::{fs::File, io, io::Write, path::Path};

use super::Prefix;

/// Relative path proven to stay inside the install prefix when joined
/// to it. Only constructible via [`ensure_entry_point_relative_path`];
/// the write helpers in this module require it, so the compiler rejects
/// any path that hasn't been validated.
#[derive(Debug, Clone)]
pub(crate) struct ValidatedRelativePath(PathBuf);

impl ValidatedRelativePath {
    pub(crate) fn into_path_buf(self) -> PathBuf {
        self.0
    }

    fn absolute_under(&self, prefix: &Path) -> PathBuf {
        prefix.join(&self.0)
    }
}

/// Defence-in-depth check for entry-point paths constructed outside the
/// parser. `Path::join` keeps `..` literal and is replaced entirely by
/// an absolute RHS, so we normalize components manually and verify the
/// result lands under `prefix`.
fn ensure_entry_point_relative_path(
    relative_path: &Path,
    prefix: &Path,
) -> Result<ValidatedRelativePath, io::Error> {
    if relative_path.is_absolute() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("entry point path is absolute: {relative_path:?}"),
        ));
    }

    let mut normalized = PathBuf::new();
    for component in relative_path.components() {
        match component {
            Component::Prefix(_) | Component::RootDir => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    format!("entry point path contains a root or drive: {relative_path:?}"),
                ));
            }
            Component::ParentDir => {
                if !normalized.pop() {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidInput,
                        format!("entry point path escapes the prefix: {relative_path:?}"),
                    ));
                }
            }
            Component::CurDir => {}
            Component::Normal(part) => normalized.push(part),
        }
    }

    // Redundant after the component walk, but makes the invariant
    // explicit and survives future changes to the normalization.
    if !prefix.join(&normalized).starts_with(prefix) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("entry point path escapes the prefix: {relative_path:?}"),
        ));
    }

    Ok(ValidatedRelativePath(normalized))
}

/// Get the bytes of the windows launcher executable.
///
/// These are the code-signed CLI launchers (proxies for `python.exe`) that
/// back Python `console_scripts` entry points on Windows. They are vendored
/// from the [`conda/conda-launchers`] signed release assets; see
/// `scripts/update-launchers.py` for how to refresh them.
///
/// [`conda/conda-launchers`]: https://github.com/conda/conda-launchers/releases
pub fn get_windows_launcher(platform: &Platform) -> &'static [u8] {
    match platform {
        Platform::Win32 => include_bytes!("../../resources/cli-32.exe"),
        Platform::Win64 => include_bytes!("../../resources/cli-64.exe"),
        Platform::WinArm64 => include_bytes!("../../resources/cli-arm64.exe"),
        _ => panic!("unsupported platform for a Windows entry point launcher: {platform}"),
    }
}

/// Creates an "entry point" on disk for a Python entrypoint. Entrypoints are executable files that
/// directly call a certain Python function.
///
/// On unix this is pretty trivial through the use of an executable shell script that invokes the
/// python compiler which in turn invokes the correct Python function. On windows however, this
/// mechanism doesn't exists. Instead a special executable is copied that starts a Python interpreter
/// which executes a file that is named the same as the executable but with the `.py` file
/// extension. So if there is an entry point file called `foo.py` an executable is created called
/// `foo.exe` that will automatically invoke `foo.py`.
///
/// The special executable is embedded in the library. The launchers are the
/// signed release binaries from <https://github.com/conda/conda-launchers> (a
/// `CPython` 3.7 launcher patched for the conda ecosystem).
///
/// See [`create_unix_python_entry_point`] for the unix variant of this function.
pub fn create_windows_python_entry_point(
    target_dir: &Prefix,
    target_prefix: &str,
    entry_point: &EntryPoint,
    python_info: &PythonInfo,
    target_platform: &Platform,
) -> Result<[PathsEntry; 2], std::io::Error> {
    let relative_path_script_py = ensure_entry_point_relative_path(
        &python_info
            .bin_dir
            .join(format!("{}-script.py", &entry_point.command)),
        target_dir.path(),
    )?;
    let relative_path_script_exe = ensure_entry_point_relative_path(
        &python_info
            .bin_dir
            .join(format!("{}.exe", &entry_point.command)),
        target_dir.path(),
    )?;

    let script_contents =
        python_entry_point_template(target_prefix, true, entry_point, python_info);
    let (hash, size) = write_validated_entry_point_file(
        target_dir.path(),
        &relative_path_script_py,
        script_contents,
    )?;

    let launcher_bytes = get_windows_launcher(target_platform);
    write_validated_entry_point_bytes(
        target_dir.path(),
        &relative_path_script_exe,
        launcher_bytes,
    )?;

    // The launcher is written verbatim, so its recorded digest is simply the
    // hash of the embedded bytes. Computing it here (rather than hardcoding)
    // keeps the record correct across platforms and whenever the launchers are
    // updated to a new signed release.
    let launcher_digest =
        rattler_digest::compute_bytes_digest::<rattler_digest::Sha256>(launcher_bytes);

    Ok([
        PathsEntry {
            relative_path: relative_path_script_py.into_path_buf(),
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
            relative_path: relative_path_script_exe.into_path_buf(),
            original_path: None,
            path_type: PathType::WindowsPythonEntryPointExe,
            no_link: false,
            sha256: Some(launcher_digest),
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
    target_dir: &Prefix,
    target_prefix: &str,
    entry_point: &EntryPoint,
    python_info: &PythonInfo,
) -> Result<PathsEntry, std::io::Error> {
    let relative_path = ensure_entry_point_relative_path(
        &python_info.bin_dir.join(&entry_point.command),
        target_dir.path(),
    )?;

    let script_contents =
        python_entry_point_template(target_prefix, false, entry_point, python_info);
    let (hash, size) =
        write_validated_entry_point_file(target_dir.path(), &relative_path, script_contents)?;

    #[cfg(unix)]
    set_validated_entry_point_executable(target_dir.path(), &relative_path)?;

    Ok(PathsEntry {
        relative_path: relative_path.into_path_buf(),
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

/// Writes `contents` to `<prefix>/<relative_path>` and returns its hash
/// and size.
fn write_validated_entry_point_file(
    prefix: &Path,
    relative_path: &ValidatedRelativePath,
    contents: impl AsRef<[u8]>,
) -> io::Result<(Output<Sha256>, usize)> {
    let absolute = relative_path.absolute_under(prefix);
    if let Some(parent) = absolute.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let bytes = contents.as_ref();
    let mut writer = HashingWriter::<_, Sha256>::new(File::create(&absolute)?);
    writer.write_all(bytes)?;
    let (_, hash) = writer.finalize();
    Ok((hash, bytes.len()))
}

/// Writes raw bytes to `<prefix>/<relative_path>` (Windows launcher).
fn write_validated_entry_point_bytes(
    prefix: &Path,
    relative_path: &ValidatedRelativePath,
    bytes: &[u8],
) -> io::Result<()> {
    let absolute = relative_path.absolute_under(prefix);
    if let Some(parent) = absolute.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(absolute, bytes)
}

/// Sets the executable bit on `<prefix>/<relative_path>`.
#[cfg(unix)]
fn set_validated_entry_point_executable(
    prefix: &Path,
    relative_path: &ValidatedRelativePath,
) -> io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(
        relative_path.absolute_under(prefix),
        std::fs::Permissions::from_mode(0o775),
    )
}

#[cfg(test)]
mod test {
    use super::ensure_entry_point_relative_path;
    use crate::install::PythonInfo;
    use rattler_conda_types::package::EntryPoint;
    use rattler_conda_types::{Platform, Version};
    use std::path::{Path, PathBuf};
    use std::str::FromStr;

    #[test]
    fn test_ensure_entry_point_relative_path_accepts_simple_name() {
        let prefix = Path::new("/opt/conda");
        let resolved = ensure_entry_point_relative_path(&Path::new("bin").join("pip"), prefix)
            .unwrap()
            .into_path_buf();
        assert_eq!(resolved, PathBuf::from("bin").join("pip"));
    }

    #[test]
    fn test_ensure_entry_point_relative_path_rejects_escape_via_parent() {
        let prefix = Path::new("/opt/conda");
        assert!(
            ensure_entry_point_relative_path(Path::new("bin/../../etc/passwd"), prefix).is_err()
        );
        assert!(ensure_entry_point_relative_path(Path::new("../etc/passwd"), prefix).is_err());
        assert!(ensure_entry_point_relative_path(Path::new(".."), prefix).is_err());
    }

    #[test]
    fn test_ensure_entry_point_relative_path_rejects_absolute() {
        let prefix = Path::new("/opt/conda");
        assert!(ensure_entry_point_relative_path(Path::new("/tmp/PWN"), prefix).is_err());
    }

    #[test]
    fn test_ensure_entry_point_relative_path_normalizes_in_prefix_traversal() {
        // The defence-in-depth layer allows `..` as long as the final
        // path lands under the prefix; the parser is what rejects it
        // on the user-facing path.
        let prefix = Path::new("/opt/conda");
        let resolved = ensure_entry_point_relative_path(Path::new("bin/../bin/pip"), prefix)
            .unwrap()
            .into_path_buf();
        assert_eq!(resolved, PathBuf::from("bin").join("pip"));
    }

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
