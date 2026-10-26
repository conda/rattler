use std::io::{Cursor, Write};

use rattler_conda_types::VersionWithSource;
use rstest::rstest;

use super::*;

fn package_record(name: &str, version: &str) -> PackageRecord {
    PackageRecord::new(
        name.parse().unwrap(),
        version.parse::<VersionWithSource>().unwrap(),
        "0".parse().unwrap(),
    )
}

fn wheel(dist_info: &str, files: &[(&str, &[u8])]) -> Vec<u8> {
    let mut writer = zip::ZipWriter::new(Cursor::new(Vec::new()));
    let options =
        zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
    writer
        .start_file(format!("{dist_info}/WHEEL"), options)
        .unwrap();
    writer.write_all(b"Wheel-Version: 1.0\n").unwrap();
    // Original RECORD contents are replaced, not used as an integrity check.
    writer
        .start_file(format!("{dist_info}/RECORD"), options)
        .unwrap();
    writer.write_all(b"missing,sha256=invalid,999\n").unwrap();
    for (name, contents) in files {
        writer.start_file(*name, options).unwrap();
        writer.write_all(contents).unwrap();
    }
    writer.finish().unwrap().into_inner()
}

#[test]
fn seekable_and_streamed_wheels_match() {
    let bytes = wheel(
        "probe-1.0.dist-info",
        &[("probe.bin", &vec![42; SPOOL_LIMIT + 1])],
    );
    let record = package_record("probe", "1.0");
    let root = tempfile::tempdir().unwrap();
    let archive = root.path().join("probe.whl");
    std::fs::write(&archive, &bytes).unwrap();
    let local = root.path().join("local");
    let streamed = root.path().join("streamed");
    let local_result = crate::fs::extract_wheel(&archive, &local, &record).unwrap();
    let stream_result = extract_wheel(bytes.as_slice(), &streamed, &record).unwrap();
    assert_eq!(local_result.sha256, stream_result.sha256);
    assert_eq!(local_result.md5, stream_result.md5);
    assert_eq!(local_result.total_size, bytes.len() as u64);
    assert_eq!(stream_result.total_size, local_result.total_size);
    for file in [
        "info/paths.json",
        "info/index.json",
        "info/link.json",
        "info/wheel.json",
    ] {
        assert_eq!(
            std::fs::read(local.join(file)).unwrap(),
            std::fs::read(streamed.join(file)).unwrap()
        );
    }
    let paths = PathsJson::from_package_directory(&local).unwrap();
    for entry in paths.paths {
        assert_eq!(
            std::fs::read(local.join(&entry.relative_path)).unwrap(),
            std::fs::read(streamed.join(&entry.relative_path)).unwrap(),
        );
    }
}

#[rstest]
#[case("other-1.0.dist-info", false)]
#[case("probe_pkg-9.9.dist-info", false)]
#[case("Probe_Pkg-1.0.0.dist-info", true)]
#[case("probe...pkg-1.0.dist-info", true)]
fn distribution_identity(#[case] dist_info: &str, #[case] valid: bool) {
    let bytes = wheel(dist_info, &[]);
    let root = tempfile::tempdir().unwrap();
    let destination = root.path().join("cache");
    let result = extract_wheel(
        bytes.as_slice(),
        &destination,
        &package_record("probe-pkg", "1.0"),
    );
    if valid {
        result.unwrap();
    } else {
        assert!(matches!(
            result,
            Err(ExtractError::WheelError(
                WheelError::IdentityMismatch { .. }
            ))
        ));
        assert!(!destination.exists());
    }
}

#[rstest]
#[case("probe-1.0.data/data/conda-meta/history")]
#[case("probe-1.0.data/data/CONDA-META/history")]
#[case("probe-1.0.data/data/info/index.json")]
fn reserved_metadata(#[case] path: &str) {
    let bytes = wheel("probe-1.0.dist-info", &[(path, b"payload")]);
    let root = tempfile::tempdir().unwrap();
    let destination = root.path().join("cache");
    assert!(matches!(
        extract_wheel(
            bytes.as_slice(),
            &destination,
            &package_record("probe", "1.0")
        ),
        Err(ExtractError::WheelError(WheelError::ReservedPath(_)))
    ));
    assert!(!destination.exists());
}

#[rstest]
#[case("probe-1.0.data/scripts/tool")]
#[case("probe-1.0.data/scripts/tool.exe")]
#[case("probe-1.0.data/scripts/tool-script.py")]
#[case("probe-1.0.data/data/bin/tool")]
#[case("probe-1.0.data/data/Scripts/tool.exe")]
fn entry_point_collisions(#[case] path: &str) {
    let bytes = wheel(
        "probe-1.0.dist-info",
        &[
            (
                "probe-1.0.dist-info/entry_points.txt",
                b"[console_scripts]\ntool = probe:main\n",
            ),
            (path, b"script"),
        ],
    );
    let root = tempfile::tempdir().unwrap();
    let destination = root.path().join("cache");
    assert!(matches!(
        extract_wheel(
            bytes.as_slice(),
            &destination,
            &package_record("probe", "1.0")
        ),
        Err(ExtractError::WheelError(WheelError::DestinationCollision(
            _
        )))
    ));
    assert!(!destination.exists());
}

#[test]
fn duplicate_entry_points_are_rejected() {
    let bytes = wheel(
        "probe-1.0.dist-info",
        &[(
            "probe-1.0.dist-info/entry_points.txt",
            b"[console_scripts]\ntool = probe:main\n[gui_scripts]\ntool = probe:gui\n",
        )],
    );
    let root = tempfile::tempdir().unwrap();
    let destination = root.path().join("cache");
    assert!(matches!(
        extract_wheel(
            bytes.as_slice(),
            &destination,
            &package_record("probe", "1.0")
        ),
        Err(ExtractError::WheelError(WheelError::DestinationCollision(
            _
        )))
    ));
    assert!(!destination.exists());
}

#[rstest]
#[case(NoArchType::none())]
#[case(NoArchType::python())]
#[case(NoArchType::generic())]
fn python_layout_and_pycache(#[case] noarch: NoArchType) {
    let bytes = wheel(
        "probe-1.0.dist-info",
        &[
            ("probe/__pycache__/module.pyc", b"bytecode"),
            ("probe/__pycache__notes.txt", b"keep"),
            ("probe-1.0.data/data/share/probe.txt", b"data"),
        ],
    );
    let mut record = package_record("probe", "1.0");
    record.noarch = noarch;
    let root = tempfile::tempdir().unwrap();
    let destination = root.path().join("cache");
    let result = extract_wheel(bytes.as_slice(), &destination, &record);
    if noarch.is_generic() {
        assert!(matches!(
            result,
            Err(ExtractError::WheelError(WheelError::GenericNoArch))
        ));
        assert!(!destination.exists());
        return;
    }
    result.unwrap();
    assert!(
        IndexJson::from_package_directory(&destination)
            .unwrap()
            .noarch
            .is_python()
    );
    assert!(!destination.join("site-packages/probe/__pycache__").exists());
    assert!(
        destination
            .join("site-packages/probe/__pycache__notes.txt")
            .is_file()
    );
    assert!(destination.join("share/probe.txt").is_file());
    let paths = PathsJson::from_package_directory(&destination).unwrap();
    assert!(!paths.paths.iter().any(|entry| {
        entry
            .relative_path
            .extension()
            .is_some_and(|extension| extension == "pyc")
    }));
}

#[test]
fn headers_use_conda_layout() {
    let bytes = wheel(
        "probe-1.0.dist-info",
        &[
            ("probe-1.0.data/headers/probe/api.h", b"header"),
            ("probe-1.0.dist-info/licenses/UNDECLARED", b"license"),
        ],
    );
    let root = tempfile::tempdir().unwrap();
    extract_wheel(
        bytes.as_slice(),
        root.path(),
        &package_record("probe", "1.0"),
    )
    .unwrap();
    assert_eq!(
        std::fs::read(root.path().join("include/probe/api.h")).unwrap(),
        b"header"
    );
    assert!(!root.path().join("info/licenses").exists());
    let paths = PathsJson::from_package_directory(root.path()).unwrap();
    assert!(
        paths
            .paths
            .iter()
            .any(|entry| entry.relative_path == Path::new("include/probe/api.h"))
    );
}

#[test]
fn declared_licenses_are_copied_as_metadata() {
    let bytes = wheel("probe-1.0.dist-info", &[
        ("probe-1.0.dist-info/METADATA", b"Metadata-Version: 2.4\r\nLicense-File: LICENSE\r\nlicense-file: docs/NOTICE\r\nLicense-File: LICENSE\r\nLicense-File: licenses/docs/NOTICE\r\nLicense-File: MISSING\r\nLicense-File:\r\n docs/THIRD_PARTY.txt\r\n\r\nLicense-File: ../ignored-description\r\n"),
        ("probe-1.0.dist-info/LICENSE", b"flat license"),
        ("probe-1.0.dist-info/licenses/LICENSE", b"other license"),
        ("probe-1.0.dist-info/licenses/docs/NOTICE", b"notice"),
        ("probe-1.0.dist-info/licenses/docs/THIRD_PARTY.txt", b"third party"),
    ]);
    let root = tempfile::tempdir().unwrap();
    extract_wheel(
        bytes.as_slice(),
        root.path(),
        &package_record("probe", "1.0"),
    )
    .unwrap();
    for (listed, source, contents) in [
        ("LICENSE", "LICENSE", b"flat license".as_slice()),
        ("docs/NOTICE", "licenses/docs/NOTICE", b"notice".as_slice()),
        (
            "docs/THIRD_PARTY.txt",
            "licenses/docs/THIRD_PARTY.txt",
            b"third party".as_slice(),
        ),
    ] {
        assert_eq!(
            std::fs::read(root.path().join("info/licenses").join(listed)).unwrap(),
            contents
        );
        assert_eq!(
            std::fs::read(
                root.path()
                    .join("site-packages/probe-1.0.dist-info")
                    .join(source)
            )
            .unwrap(),
            contents
        );
    }
    assert!(!root.path().join("info/licenses/licenses").exists());
    assert!(!root.path().join("info/licenses/MISSING").exists());
    let paths = PathsJson::from_package_directory(root.path()).unwrap();
    assert!(
        !paths
            .paths
            .iter()
            .any(|entry| entry.relative_path.starts_with("info"))
    );
    let record =
        std::fs::read_to_string(root.path().join("site-packages/probe-1.0.dist-info/RECORD"))
            .unwrap();
    assert!(!record.contains("../info/licenses/"));
    assert!(record.contains("probe-1.0.dist-info/licenses/docs/NOTICE,"));
}

#[rstest]
#[case("../LICENSE")]
#[case("/LICENSE")]
#[case("C:/LICENSE")]
#[case("..\\LICENSE")]
#[case("docs//LICENSE")]
fn unsafe_license_paths_are_rejected(#[case] path: &str) {
    let metadata = format!("License-File: {path}\n");
    let bytes = wheel(
        "probe-1.0.dist-info",
        &[("probe-1.0.dist-info/METADATA", metadata.as_bytes())],
    );
    let root = tempfile::tempdir().unwrap();
    let destination = root.path().join("cache");
    assert!(matches!(
        extract_wheel(
            bytes.as_slice(),
            &destination,
            &package_record("probe", "1.0")
        ),
        Err(ExtractError::InvalidArchivePath(_))
    ));
    assert!(!destination.exists());
}

#[test]
fn conflicting_license_destinations_are_rejected() {
    let bytes = wheel(
        "probe-1.0.dist-info",
        &[
            (
                "probe-1.0.dist-info/METADATA",
                b"License-File: docs\nLicense-File: docs/NOTICE\n",
            ),
            ("probe-1.0.dist-info/docs", b"flat license"),
            ("probe-1.0.dist-info/licenses/docs/NOTICE", b"notice"),
        ],
    );
    let root = tempfile::tempdir().unwrap();
    let destination = root.path().join("cache");
    assert!(matches!(
        extract_wheel(
            bytes.as_slice(),
            &destination,
            &package_record("probe", "1.0")
        ),
        Err(ExtractError::WheelError(WheelError::DestinationCollision(
            _
        )))
    ));
    assert!(!destination.exists());
}

#[cfg(unix)]
#[test]
fn permissions_respect_umask() {
    use std::os::unix::fs::PermissionsExt;

    const CHILD: &str = "RATTLER_WHEEL_TEST_UMASK";
    if std::env::var_os(CHILD).is_none() {
        // Set umask only in a subprocess, never in the parallel test runner.
        let status = std::process::Command::new("sh")
            .args([
                "-c",
                "umask 077; exec \"$1\" --exact wheel::tests::permissions_respect_umask",
                "wheel-permissions",
            ])
            .arg(std::env::current_exe().unwrap())
            .env(CHILD, "1")
            .status()
            .unwrap();
        assert!(status.success());
        return;
    }
    let root = tempfile::tempdir().unwrap();
    for (name, mode, expected) in [("executable", 0o777, 0o700), ("regular", 0o666, 0o600)] {
        write_entry(root.path(), name.into(), b"payload".as_slice(), Some(mode)).unwrap();
        assert_eq!(
            std::fs::metadata(root.path().join(name))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            expected
        );
    }
}
