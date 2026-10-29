use std::{
    collections::BTreeSet,
    path::{Component, Path, PathBuf},
};

use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use rattler_cache::package_cache::PackageCache;
use rattler_conda_types::{
    NoArchType, PackageRecord, PrefixRecord, RepoDataRecord, Subdir, Version,
    package::{DistArchiveIdentifier, PackageFile, PathsJson},
    prefix_record::PathType,
};
use rattler_digest::{Sha256, compute_bytes_digest};
use url::Url;

use crate::install::{Installer, LinkOptions, PythonInfo};

fn wheel(name: &str) -> RepoDataRecord {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../test-data/wheels")
        .join(format!("{}-1.0-py3-none-any.whl", name.replace('-', "_")));
    let identifier = DistArchiveIdentifier::try_from_path(&path).unwrap();
    RepoDataRecord {
        package_record: PackageRecord {
            subdir: "noarch".into(),
            noarch: NoArchType::python(),
            depends: vec!["python >=3".into()],
            ..PackageRecord::new(
                name.parse().unwrap(),
                "1.0".parse::<Version>().unwrap(),
                identifier.identifier.build_string.parse().unwrap(),
            )
        },
        identifier,
        url: Url::from_file_path(path).unwrap(),
        channel: None,
    }
}

async fn python(
    cache: &PackageCache,
    platform: Subdir,
    version: &str,
    custom: Option<&str>,
    collision: bool,
    gui: bool,
) -> RepoDataRecord {
    let record = RepoDataRecord {
        package_record: PackageRecord {
            subdir: platform.to_string(),
            python_site_packages_path: custom.map(str::to_owned),
            ..PackageRecord::new(
                "python".parse().unwrap(),
                version.parse::<Version>().unwrap(),
                "0".parse().unwrap(),
            )
        },
        identifier: DistArchiveIdentifier::try_from_filename(&format!("python-{version}-0.conda"))
            .unwrap(),
        url: format!("https://example.invalid/python-{version}-0.conda")
            .parse()
            .unwrap(),
        channel: None,
    };
    let info = PythonInfo::from_python_record(&record.package_record, platform).unwrap();
    let mut files = vec![info.path.clone()];
    if platform.is_windows() {
        if gui {
            files.push("pythonw.exe".into());
        }
    } else {
        files.push("bin/python".into());
    }
    if collision {
        files.push(info.bin_dir.join(if platform.is_windows() {
            "record-probe.exe"
        } else {
            "record-probe"
        }));
    }
    let index = serde_json::to_vec(&record.package_record).unwrap();
    cache.get_or_fetch(&record.package_record, move |path| {
        let (files, index) = (files.clone(), index.clone());
        async move {
            std::fs::create_dir_all(path.join("info"))?;
            std::fs::write(path.join("info/index.json"), index)?;
            for file in &files {
                std::fs::create_dir_all(path.join(file).parent().unwrap())?;
                std::fs::write(path.join(file), b"interpreter fixture")?;
            }
            let entries: Vec<_> = files.iter().map(|file| serde_json::json!({"_path":file.to_str().unwrap().replace('\\',"/"), "path_type":"hardlink"})).collect();
            std::fs::write(path.join("info/paths.json"), serde_json::to_vec(&serde_json::json!({"paths_version":1,"paths":entries}))?)?;
            Ok::<_,std::io::Error>(())
        }
    }, None).await.unwrap();
    record
}

async fn install(
    prefix: &Path,
    cache: &PackageCache,
    platform: Subdir,
    records: Vec<RepoDataRecord>,
) -> crate::install::installer::InstallationResult {
    Installer::new()
        .with_package_cache(cache.clone())
        .with_target_platform(platform)
        .with_link_options(LinkOptions {
            allow_ref_links: Some(false),
            ..Default::default()
        })
        .with_download_client(
            reqwest_middleware::ClientBuilder::new(reqwest::Client::new())
                .with(rattler_networking::OfflineMiddleware)
                .build(),
        )
        .install(prefix, records)
        .await
        .unwrap()
}

fn normalized(path: &Path) -> PathBuf {
    let mut result = PathBuf::new();
    for component in path.components() {
        if component == Component::ParentDir {
            assert!(result.pop());
        } else if component != Component::CurDir {
            result.push(component.as_os_str());
        }
    }
    result
}

fn assert_record(prefix: &Path, name: &str) -> PrefixRecord {
    let records: Vec<PrefixRecord> = PrefixRecord::collect_from_prefix(prefix).unwrap();
    let record = records
        .into_iter()
        .find(|record| record.repodata_record.package_record.name.as_normalized() == name)
        .unwrap();
    let record_entry = record
        .paths_data
        .paths
        .iter()
        .find(|entry| {
            entry
                .relative_path
                .file_name()
                .is_some_and(|name| name == "RECORD")
        })
        .unwrap();
    let destination = prefix.join(&record_entry.relative_path);
    let bytes = std::fs::read(&destination).unwrap();
    let base = destination.parent().unwrap().parent().unwrap();
    let mut reader = csv::ReaderBuilder::new()
        .has_headers(false)
        .from_reader(bytes.as_slice());
    let mut listed = BTreeSet::new();
    for row in reader.records() {
        let row = row.unwrap();
        assert_eq!(row.len(), 3);
        let path = normalized(&base.join(&row[0]));
        assert!(path.starts_with(prefix));
        assert!(listed.insert(path.clone()), "duplicate RECORD row");
        let content = std::fs::read(&path).unwrap();
        if path == destination {
            assert_eq!((&row[1], &row[2]), ("", ""));
        } else {
            assert_eq!(
                row[1],
                format!(
                    "sha256={}",
                    URL_SAFE_NO_PAD.encode(compute_bytes_digest::<Sha256>(&content))
                )
            );
            assert_eq!(row[2], content.len().to_string());
        }
    }
    let owned = record
        .paths_data
        .paths
        .iter()
        .filter(|entry| entry.path_type != PathType::Directory)
        .map(|entry| prefix.join(&entry.relative_path))
        .collect();
    assert_eq!(listed, owned);
    assert_eq!(
        record_entry.sha256_in_prefix.or(record_entry.sha256),
        Some(compute_bytes_digest::<Sha256>(&bytes))
    );
    assert_eq!(record_entry.size_in_bytes, Some(bytes.len() as u64));
    record
}

#[tokio::test]
async fn installed_record_layout_cache_and_retry() {
    for (platform, custom) in [
        (Subdir::Linux64, None),
        (Subdir::Win64, None),
        (Subdir::Linux64, Some("custom/packages")),
    ] {
        let root = tempfile::tempdir().unwrap();
        let cache = PackageCache::new(root.path().join("cache"));
        let wheel = wheel("record-probe");
        let cached = cache
            .get_or_fetch_from_path(
                &wheel.url.to_file_path().unwrap(),
                Some(&wheel.package_record),
                None,
            )
            .await
            .unwrap();
        let snapshot: Vec<_> = PathsJson::from_package_directory(cached.path())
            .unwrap()
            .paths
            .iter()
            .map(|entry| {
                let path = cached.path().join(&entry.relative_path);
                (
                    path.clone(),
                    std::fs::read(&path).unwrap(),
                    std::fs::metadata(path).unwrap().permissions(),
                )
            })
            .collect();
        let python = python(&cache, platform, "3.12.0", custom, false, true).await;
        for folder in ["environment", "environment with spaces"] {
            let target = root.path().join(folder);
            install(
                &target,
                &cache,
                platform,
                vec![python.clone(), wheel.clone()],
            )
            .await;
            let mut installed = assert_record(&target, "record-probe");
            let info = PythonInfo::from_python_record(&python.package_record, platform).unwrap();
            assert!(
                installed
                    .paths_data
                    .paths
                    .iter()
                    .any(|entry| entry.relative_path
                        == info.bin_dir.join(if platform.is_windows() {
                            "record-probe.exe"
                        } else {
                            "record-probe"
                        }))
            );
            // Simulate metadata persistence failing after RECORD replacement.
            let entry = installed
                .paths_data
                .paths
                .iter_mut()
                .find(|entry| {
                    entry
                        .relative_path
                        .file_name()
                        .is_some_and(|name| name == "RECORD")
                })
                .unwrap();
            entry.sha256_in_prefix = None;
            entry.size_in_bytes = Some(0);
            installed
                .write_to_path(target.join("conda-meta").join(installed.file_name()), true)
                .unwrap();
            let retry = install(
                &target,
                &cache,
                platform,
                vec![python.clone(), wheel.clone()],
            )
            .await;
            assert!(retry.transaction.operations.is_empty());
            let mut moved = assert_record(&target, "record-probe");
            // Exercise an already-clobbered RECORD whose payload remains at
            // other destinations. Relative rows must use its actual location.
            let entry = moved
                .paths_data
                .paths
                .iter_mut()
                .find(|entry| {
                    entry
                        .relative_path
                        .file_name()
                        .is_some_and(|name| name == "RECORD")
                })
                .unwrap();
            let original = entry.relative_path.clone();
            let relocated = Path::new(crate::install::clobber_registry::CLOBBERS_DIR_NAME)
                .join("record-probe")
                .join(&original);
            std::fs::create_dir_all(target.join(&relocated).parent().unwrap()).unwrap();
            std::fs::rename(target.join(&original), target.join(&relocated)).unwrap();
            entry.original_path = Some(original.clone());
            entry.relative_path = relocated.clone();
            for file in &mut moved.files {
                if *file == original {
                    *file = relocated.clone();
                }
            }
            moved
                .write_to_path(target.join("conda-meta").join(moved.file_name()), true)
                .unwrap();
            install(
                &target,
                &cache,
                platform,
                vec![python.clone(), wheel.clone()],
            )
            .await;
            assert_record(&target, "record-probe");
        }
        for (path, bytes, mode) in snapshot {
            assert_eq!(std::fs::read(&path).unwrap(), bytes);
            assert_eq!(std::fs::metadata(path).unwrap().permissions(), mode);
        }
    }
}

#[tokio::test]
async fn installed_record_tracks_entry_point_clobbers() {
    for platform in [Subdir::Linux64, Subdir::Win64] {
        let root = tempfile::tempdir().unwrap();
        let cache = PackageCache::new(root.path().join("cache"));
        let python = python(&cache, platform, "3.12.0", None, true, true).await;
        let target = root.path().join("environment");
        let first = wheel("record-probe");
        let second = wheel("record-other");
        // Conda distinguishes these spellings; wheel name normalization does
        // not. This exercises a real collision of the dist-info files too.
        let mut alias = first.clone();
        alias.package_record.name = "record_probe".parse().unwrap();
        install(
            &target,
            &cache,
            platform,
            vec![python.clone(), first.clone(), second.clone(), alias],
        )
        .await;
        let first_record = assert_record(&target, "record-probe");
        let second_record = assert_record(&target, "record-other");
        let alias_record = assert_record(&target, "record_probe");
        assert!(
            first_record
                .paths_data
                .paths
                .iter()
                .chain(&alias_record.paths_data.paths)
                .any(|entry| entry
                    .relative_path
                    .file_name()
                    .is_some_and(|name| name == "RECORD")
                    && entry.original_path.is_some())
        );
        let first_files: BTreeSet<_> = first_record
            .paths_data
            .paths
            .iter()
            .map(|entry| &entry.relative_path)
            .collect();
        assert!(
            second_record
                .paths_data
                .paths
                .iter()
                .all(|entry| !first_files.contains(&entry.relative_path))
        );
        assert!(
            first_record
                .paths_data
                .paths
                .iter()
                .chain(&second_record.paths_data.paths)
                .any(|entry| entry.original_path.is_some())
        );
        // Removing the winner can move an unchanged wheel's command out of the
        // clobber directory. Its RECORD must be refreshed too.
        install(&target, &cache, platform, vec![python, first]).await;
        assert_record(&target, "record-probe");
    }
}

#[tokio::test]
async fn installed_record_rejects_legacy_ambiguous_command_ownership() {
    let root = tempfile::tempdir().unwrap();
    let cache = PackageCache::new(root.path().join("cache"));
    let target = root.path().join("environment");
    let old_python = python(&cache, Subdir::Linux64, "3.12.0", None, false, true).await;
    let first = wheel("record-probe");
    let second = wheel("record-other");
    install(
        &target,
        &cache,
        Subdir::Linux64,
        vec![old_python.clone(), first.clone(), second.clone()],
    )
    .await;
    let mut records: Vec<PrefixRecord> = PrefixRecord::collect_from_prefix(&target).unwrap();
    let loser = records
        .iter_mut()
        .find(|record| {
            record.paths_data.paths.iter().any(|entry| {
                entry.path_type == PathType::UnixPythonEntryPoint && entry.original_path.is_some()
            })
        })
        .unwrap();
    let entry = loser
        .paths_data
        .paths
        .iter_mut()
        .find(|entry| entry.path_type == PathType::UnixPythonEntryPoint)
        .unwrap();
    let previous = entry.relative_path.clone();
    let shared = entry.original_path.take().unwrap();
    entry.relative_path = shared.clone();
    *loser
        .files
        .iter_mut()
        .find(|path| **path == previous)
        .unwrap() = shared;
    std::fs::remove_file(target.join(previous)).unwrap();
    let metadata = target.join("conda-meta").join(loser.file_name());
    loser.write_to_path(&metadata, true).unwrap();
    let record_path = target.join(
        &loser
            .paths_data
            .paths
            .iter()
            .find(|entry| {
                entry
                    .relative_path
                    .file_name()
                    .is_some_and(|name| name == "RECORD")
            })
            .unwrap()
            .relative_path,
    );
    let snapshot: Vec<_> = [metadata, record_path]
        .into_iter()
        .map(|path| {
            let bytes = std::fs::read(&path).unwrap();
            (path, bytes)
        })
        .collect();
    let new_python = python(&cache, Subdir::Linux64, "3.12.1", None, false, true).await;
    // Both no-op recovery and ordinary post-processing must reject the legacy
    // ambiguity before moving files or manufacturing new RECORD contents.
    for interpreter in [old_python, new_python] {
        let error = Installer::new()
            .with_package_cache(cache.clone())
            .with_target_platform(Subdir::Linux64)
            .install(&target, vec![interpreter, first.clone(), second.clone()])
            .await
            .unwrap_err();
        assert!(format!("{error:?}").contains("ambiguous wheel file ownership"));
        for (path, bytes) in &snapshot {
            assert_eq!(&std::fs::read(path).unwrap(), bytes);
        }
    }
}

#[tokio::test]
async fn installed_record_follows_python_changes_and_ignores_exclusions() {
    for platform in [Subdir::Linux64, Subdir::Win64] {
        let root = tempfile::tempdir().unwrap();
        let cache = PackageCache::new(root.path().join("cache"));
        let target = root.path().join("environment");
        let wheel = wheel("record-probe");
        let old_python = python(&cache, platform, "3.12.0", None, false, true).await;
        install(&target, &cache, platform, vec![old_python, wheel.clone()]).await;
        let mut current_python = python(&cache, platform, "3.13.0", None, false, true).await;
        install(
            &target,
            &cache,
            platform,
            vec![current_python.clone(), wheel.clone()],
        )
        .await;
        assert_record(&target, "record-probe");
        if platform.is_windows() {
            for (version, gui) in [("3.13.1", false), ("3.13.2", true)] {
                current_python = python(&cache, platform, version, None, false, gui).await;
                let result = install(
                    &target,
                    &cache,
                    platform,
                    vec![current_python.clone(), wheel.clone()],
                )
                .await;
                assert_eq!(result.transaction.operations.len(), 1);
                assert_record(&target, "record-probe");
            }
        }
        install(&target, &cache, platform, vec![wheel.clone()]).await;
        assert_record(&target, "record-probe");
        let retry = install(&target, &cache, platform, vec![wheel.clone()]).await;
        assert!(retry.transaction.operations.is_empty());
        let record = assert_record(&target, "record-probe");
        let metadata_path = target.join("conda-meta").join(record.file_name());
        let before = std::fs::read(&metadata_path).unwrap();
        let record_path = target.join(
            &record
                .paths_data
                .paths
                .iter()
                .find(|entry| {
                    entry
                        .relative_path
                        .file_name()
                        .is_some_and(|name| name == "RECORD")
                })
                .unwrap()
                .relative_path,
        );
        std::fs::write(&record_path, b"deliberately ignored\n").unwrap();
        Installer::new()
            .with_package_cache(cache)
            .with_target_platform(platform)
            .with_ignored_packages([wheel.package_record.name.clone()].into_iter().collect())
            .install(&target, vec![current_python, wheel])
            .await
            .unwrap();
        assert_eq!(std::fs::read(metadata_path).unwrap(), before);
        assert_eq!(
            std::fs::read(record_path).unwrap(),
            b"deliberately ignored\n"
        );
    }
}

#[tokio::test]
async fn installed_record_uses_owned_paths_for_platform_wheels() {
    let root = tempfile::tempdir().unwrap();
    let cache = PackageCache::new(root.path().join("cache"));
    let target = root.path().join("environment");
    let mut wheel = wheel("record-probe");
    wheel.package_record.noarch = NoArchType::default();
    wheel.package_record.subdir = Subdir::Linux64.to_string();
    let old_python = python(&cache, Subdir::Linux64, "3.12.0", None, false, true).await;
    install(
        &target,
        &cache,
        Subdir::Linux64,
        vec![old_python, wheel.clone()],
    )
    .await;
    let before = assert_record(&target, "record-probe");
    let new_python = python(&cache, Subdir::Linux64, "3.13.0", None, false, true).await;
    // Platform records do not request noarch relinking. RECORD must describe
    // actual owned destinations, not invent a layout from the new interpreter.
    let result = install(&target, &cache, Subdir::Linux64, vec![new_python, wheel]).await;
    assert_eq!(result.transaction.operations.len(), 1);
    let after = assert_record(&target, "record-probe");
    assert_eq!(before.files, after.files);
}

#[tokio::test]
async fn installed_record_identifies_distribution_and_rejects_ambiguity() {
    let root = tempfile::tempdir().unwrap();
    let cache = PackageCache::new(root.path().join("cache"));
    let target = root.path().join("environment");
    let python = python(&cache, Subdir::Linux64, "3.12.0", None, false, true).await;
    let wheel = wheel("record-probe");
    install(
        &target,
        &cache,
        Subdir::Linux64,
        vec![python.clone(), wheel.clone()],
    )
    .await;
    let mut record = assert_record(&target, "record-probe");
    let entry = record
        .paths_data
        .paths
        .iter()
        .find(|entry| {
            entry
                .relative_path
                .file_name()
                .is_some_and(|name| name == "RECORD")
        })
        .unwrap()
        .clone();
    let main_record = target.join(&entry.relative_path);
    let mut nested = entry;
    nested.relative_path =
        "lib/python3.12/site-packages/record_probe/fixtures/record_probe-0.9.dist-info/RECORD"
            .into();
    nested.sha256 = Some(compute_bytes_digest::<Sha256>(b"fixture"));
    nested.sha256_in_prefix = None;
    nested.size_in_bytes = Some(7);
    let nested_path = nested.relative_path.clone();
    std::fs::create_dir_all(target.join(&nested_path).parent().unwrap()).unwrap();
    std::fs::write(target.join(&nested_path), b"fixture").unwrap();
    record.files.push(nested_path.clone());
    record.paths_data.paths.push(nested);
    record
        .write_to_path(target.join("conda-meta").join(record.file_name()), true)
        .unwrap();
    install(
        &target,
        &cache,
        Subdir::Linux64,
        vec![python.clone(), wheel.clone()],
    )
    .await;
    let mut record = assert_record(&target, "record-probe");
    let expected = std::fs::read(&main_record).unwrap();
    let ambiguous: PathBuf =
        "lib/python3.12/site-packages/record_probe/fixtures/record_probe-1.0.dist-info/RECORD"
            .into();
    std::fs::create_dir_all(target.join(&ambiguous).parent().unwrap()).unwrap();
    std::fs::rename(target.join(&nested_path), target.join(&ambiguous)).unwrap();
    record
        .paths_data
        .paths
        .iter_mut()
        .find(|entry| entry.relative_path == nested_path)
        .unwrap()
        .relative_path = ambiguous.clone();
    *record
        .files
        .iter_mut()
        .find(|path| **path == nested_path)
        .unwrap() = ambiguous;
    record
        .write_to_path(target.join("conda-meta").join(record.file_name()), true)
        .unwrap();
    let error = Installer::new()
        .with_package_cache(cache)
        .with_target_platform(Subdir::Linux64)
        .install(&target, vec![python, wheel])
        .await
        .unwrap_err();
    assert!(format!("{error:?}").contains("unambiguous wheel RECORD"));
    assert_eq!(std::fs::read(main_record).unwrap(), expected);
}

#[cfg(unix)]
#[tokio::test]
async fn installed_record_observes_post_link_mutations() {
    let platform = Subdir::current().unwrap();
    let root = tempfile::tempdir().unwrap();
    let cache = PackageCache::new(root.path().join("cache"));
    let target = root.path().join("environment");
    let python = python(&cache, platform, "3.12.0", None, false, true).await;
    std::fs::create_dir_all(target.join("bin")).unwrap();
    std::fs::write(target.join("bin/.python-post-link.sh"), b"#!/bin/sh\nprintf 'changed by hook' > \"$PREFIX/share/record-probe/new.txt\"\nmv \"$PREFIX/share/record-probe/new.txt\" \"$PREFIX/share/record-probe/data.txt\"\n").unwrap();
    let result = Installer::new()
        .with_package_cache(cache)
        .with_target_platform(platform)
        .with_execute_link_scripts(true)
        .install(&target, vec![python, wheel("record-probe")])
        .await
        .unwrap();
    assert!(
        result
            .post_link_script_result
            .unwrap()
            .unwrap()
            .failed_packages
            .is_empty()
    );
    assert_eq!(
        std::fs::read(target.join("share/record-probe/data.txt")).unwrap(),
        b"changed by hook"
    );
    assert_record(&target, "record-probe");
}

#[tokio::test]
async fn installed_record_rejects_intra_package_entry_point_collisions() {
    use crate::install::{InstallDriver, InstallOptions, link_package};
    use rattler_conda_types::{
        package::{LinkJson, NoArchLinks},
        prefix::Prefix,
    };
    let root = tempfile::tempdir().unwrap();
    let cache = PackageCache::new(root.path().join("cache"));
    let wheel = wheel("record-probe");
    let cached = cache
        .get_or_fetch_from_path(
            &wheel.url.to_file_path().unwrap(),
            Some(&wheel.package_record),
            None,
        )
        .await
        .unwrap();
    let python = python(&cache, Subdir::Linux64, "3.12.0", None, false, true).await;
    let mut link = LinkJson::from_package_directory(cached.path()).unwrap();
    let NoArchLinks::Python(ref mut entries) = link.noarch else {
        panic!("expected Python link metadata")
    };
    entries.entry_points[0].command = "raw-probe".into();
    let prefix = Prefix::create(root.path().join("environment")).unwrap();
    let error = link_package(
        cached.path(),
        &prefix,
        &InstallDriver::default(),
        InstallOptions {
            platform: Some(Subdir::Linux64),
            link_json: Some(Some(link)),
            python_info: Some(
                PythonInfo::from_python_record(&python.package_record, Subdir::Linux64).unwrap(),
            ),
            ..Default::default()
        },
    )
    .await
    .unwrap_err();
    assert!(matches!(
        error,
        crate::install::InstallError::FailedToCreatePythonEntryPoint(_)
    ));
    assert!(!prefix.join("bin/raw-probe").exists());
    assert!(
        !prefix
            .join("lib/python3.12/site-packages/record_probe/__init__.py")
            .exists()
    );
}

#[cfg(unix)]
#[tokio::test]
async fn installed_record_recovers_from_write_failures() {
    use std::os::unix::fs::PermissionsExt;
    for fail_metadata in [false, true] {
        let root = tempfile::tempdir().unwrap();
        let cache = PackageCache::new(root.path().join("cache"));
        let target = root.path().join("environment");
        let python = python(&cache, Subdir::Linux64, "3.12.0", None, false, true).await;
        let wheel = wheel("record-probe");
        install(
            &target,
            &cache,
            Subdir::Linux64,
            vec![python.clone(), wheel.clone()],
        )
        .await;
        let mut record = assert_record(&target, "record-probe");
        let entry = record
            .paths_data
            .paths
            .iter_mut()
            .find(|entry| {
                entry
                    .relative_path
                    .file_name()
                    .is_some_and(|name| name == "RECORD")
            })
            .unwrap();
        let destination = target.join(&entry.relative_path);
        let expected = std::fs::read(&destination).unwrap();
        std::fs::write(&destination, b"unfinished RECORD\n").unwrap();
        entry.sha256_in_prefix = None;
        entry.size_in_bytes = Some(0);
        record
            .write_to_path(target.join("conda-meta").join(record.file_name()), true)
            .unwrap();
        let directory = if fail_metadata {
            target.join("conda-meta")
        } else {
            destination.parent().unwrap().to_owned()
        };
        let permissions = std::fs::metadata(&directory).unwrap().permissions();
        std::fs::set_permissions(&directory, std::fs::Permissions::from_mode(0o555)).unwrap();
        // Privileged test runners can bypass mode bits; retain the portable
        // stale-metadata regression above rather than asserting a false failure.
        if std::fs::write(directory.join("permission-probe"), b"").is_ok() {
            std::fs::set_permissions(&directory, permissions).unwrap();
            std::fs::remove_file(directory.join("permission-probe")).unwrap();
            continue;
        }
        let result = Installer::new()
            .with_package_cache(cache.clone())
            .with_target_platform(Subdir::Linux64)
            .install(&target, vec![python.clone(), wheel.clone()])
            .await;
        std::fs::set_permissions(&directory, permissions).unwrap();
        assert!(result.is_err());
        assert_eq!(
            std::fs::read(&destination).unwrap(),
            if fail_metadata {
                expected
            } else {
                b"unfinished RECORD\n".to_vec()
            }
        );
        let retry = install(
            &target,
            &cache,
            Subdir::Linux64,
            vec![python.clone(), wheel.clone()],
        )
        .await;
        assert!(retry.transaction.operations.is_empty());
        assert_record(&target, "record-probe");
        // Known hashes must not cause unconditional payload reads on no-op
        // installs. Replace the hardlink before changing permissions so the
        // shared cache is unaffected by this test.
        let data = target.join("share/record-probe/data.txt");
        let bytes = std::fs::read(&data).unwrap();
        let permissions = std::fs::metadata(&data).unwrap().permissions();
        let mut temporary = tempfile::NamedTempFile::new_in(data.parent().unwrap()).unwrap();
        std::io::Write::write_all(&mut temporary, &bytes).unwrap();
        temporary.persist(&data).unwrap();
        std::fs::set_permissions(&data, std::fs::Permissions::from_mode(0o000)).unwrap();
        assert!(std::fs::read(&data).is_err());
        let result = Installer::new()
            .with_package_cache(cache)
            .with_target_platform(Subdir::Linux64)
            .install(&target, vec![python, wheel])
            .await;
        std::fs::set_permissions(data, permissions).unwrap();
        assert!(result.unwrap().transaction.operations.is_empty());
    }
}

#[tokio::test]
async fn installed_record_pip_show_and_uninstall() {
    use std::process::Command;
    let platform = Subdir::current().unwrap();
    let source = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../test-data/python")
        .join(format!("explicit-env-{platform}.txt"));
    let spec = rattler_conda_types::ExplicitEnvironmentSpec::from_path(&source).unwrap();
    let root = tempfile::tempdir().unwrap();
    let cache = PackageCache::new(root.path().join("cache"));
    let python = python(&cache, platform, "3.11.0", None, false, true).await;
    let info = PythonInfo::from_python_record(&python.package_record, platform).unwrap();
    let wheel = wheel("record-probe");
    let cached = cache
        .get_or_fetch_from_path(
            &wheel.url.to_file_path().unwrap(),
            Some(&wheel.package_record),
            None,
        )
        .await
        .unwrap();
    let snapshot: Vec<_> = PathsJson::from_package_directory(cached.path())
        .unwrap()
        .paths
        .into_iter()
        .map(|entry| {
            let path = cached.path().join(entry.relative_path);
            let bytes = std::fs::read(&path).unwrap();
            (path, bytes)
        })
        .collect();
    for copy in [false, true] {
        let environment = crate::install::test::test_install_python(
            spec.packages.iter().map(|package| package.url.clone()),
            "wheel-record-python",
            platform,
        )
        .await;
        let target = environment.path();
        // Bootstrap installs through the low-level linker. Describe the existing
        // interpreter to the full installer without relinking its real files.
        let existing = PrefixRecord::from_repodata_record(python.clone(), Vec::new());
        existing
            .write_to_path(target.join("conda-meta").join(existing.file_name()), true)
            .unwrap();
        Installer::new()
            .with_package_cache(cache.clone())
            .with_target_platform(platform)
            .with_link_options(LinkOptions {
                allow_hard_links: Some(!copy),
                allow_symbolic_links: Some(true),
                allow_ref_links: Some(false),
            })
            .install(target, vec![python.clone(), wheel.clone()])
            .await
            .unwrap();
        #[cfg(unix)]
        if copy {
            // Regular wheel files are normally copied or hardlinked, not
            // symlinked. Also exercise recovery from cached-file symlinks.
            for relative in [
                "record_probe/__init__.py",
                "record_probe-1.0.dist-info/RECORD",
            ] {
                let destination = target.join(&info.site_packages_path).join(relative);
                std::fs::remove_file(&destination).unwrap();
                std::os::unix::fs::symlink(
                    cached.path().join("site-packages").join(relative),
                    destination,
                )
                .unwrap();
            }
            let retry = install(
                target,
                &cache,
                platform,
                vec![python.clone(), wheel.clone()],
            )
            .await;
            assert!(retry.transaction.operations.is_empty());
        }
        let installed = assert_record(target, "record-probe");
        assert!(
            !std::fs::symlink_metadata(
                target
                    .join(&info.site_packages_path)
                    .join("record_probe-1.0.dist-info/RECORD")
            )
            .unwrap()
            .file_type()
            .is_symlink()
        );
        let sentinel = target.join("unrelated.txt");
        std::fs::write(&sentinel, b"keep me").unwrap();
        let command = target.join(&info.bin_dir).join(if platform.is_windows() {
            "record-probe.exe"
        } else {
            "record-probe"
        });
        let output = Command::new(command)
            .env_remove("PYTHONPATH")
            .env_remove("PYTHONHOME")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(
            String::from_utf8_lossy(&output.stdout).trim(),
            "record_probe"
        );
        for arguments in [
            vec!["show", "-f", "record-probe"],
            vec!["uninstall", "-y", "record-probe"],
        ] {
            let output = Command::new(target.join(&info.path))
                .args(["-m", "pip"])
                .args(arguments)
                .env("PIP_DISABLE_PIP_VERSION_CHECK", "1")
                .env("PYTHONNOUSERSITE", "1")
                .env_remove("PYTHONPATH")
                .env_remove("PYTHONHOME")
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "stdout: {}\nstderr: {}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
        }
        for entry in &installed.paths_data.paths {
            assert!(
                !target.join(&entry.relative_path).try_exists().unwrap(),
                "left behind: {}",
                entry.relative_path.display()
            );
        }
        assert!(sentinel.exists());
        // pip deliberately does not synchronize conda's independent database.
        assert!(
            target
                .join("conda-meta")
                .join(installed.file_name())
                .exists()
        );
        for (path, bytes) in &snapshot {
            assert_eq!(&std::fs::read(path).unwrap(), bytes);
        }
    }
}
