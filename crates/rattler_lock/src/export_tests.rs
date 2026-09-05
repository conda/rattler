use std::sync::Arc;

use rattler_conda_types::{PackageName, PackageRecord};
use url::Url;

use crate::{
    CondaBinaryData, CondaPackageData, CondaSourceData, FindLinksUrlOrPath, LockFile,
    LockFileInner, LockedPackage, PackageBuildSource, PypiDistributionData, PypiIndexes,
    PypiPackageData, PypiSourceData, SourceMetadata, UrlOrPath, Verbatim,
    source::{GitSourceLocation, PathSourceLocation, SourceLocation, UrlSourceLocation},
};

const SAFE: &str = "https://example.com/demo-1-0.conda";
const UNSAFE: &str = "https://example.com/demo?opaque=EXAMPLE_SECRET";

fn url(text: &str) -> Url {
    text.parse().unwrap()
}

fn record() -> PackageRecord {
    PackageRecord::new(
        PackageName::new_unchecked("demo"),
        "1".parse::<rattler_conda_types::Version>().unwrap(),
        "0".to_owned(),
    )
}

fn with_package(package: LockedPackage) -> LockFile {
    LockFile {
        inner: Arc::new(LockFileInner {
            packages: vec![package],
            ..Default::default()
        }),
    }
}

fn conda() -> CondaBinaryData {
    CondaBinaryData {
        package_record: record(),
        location: url(SAFE).into(),
        file_name: "demo-1-0.conda".parse().unwrap(),
        channel: None,
    }
}

fn pypi() -> PypiDistributionData {
    PypiDistributionData {
        name: "demo".parse().unwrap(),
        version: "1".parse().unwrap(),
        location: Verbatim::new(url(SAFE).into()),
        index_url: None,
        hash: None,
        requires_dist: vec![],
        requires_python: None,
    }
}

fn conda_source() -> CondaSourceData {
    CondaSourceData {
        location: UrlOrPath::Path(".".into()),
        package_build_source: None,
        variants: std::collections::BTreeMap::new(),
        identifier_hash: None,
        sources: std::collections::BTreeMap::new(),
        source_data: crate::SourceData::default(),
        metadata: SourceMetadata::Full(Box::new(record())),
    }
}

fn rejected(lock: &LockFile, location: &str) {
    let error = lock.validate_urls_for_export().unwrap_err();
    assert_eq!(error.location, location);
    assert!(!format!("{error:?}: {error}").contains("EXAMPLE_SECRET"));
}

#[test]
fn channel_validation_is_opt_in_and_does_not_change_serialization() {
    for value in [
        "https://EXAMPLE_SECRET@example.com/channel",
        "https://user:EXAMPLE_SECRET@example.com/channel",
        "https://example.com/t/EXAMPLE_SECRET/channel",
        "https://example.com/%74/EXAMPLE_SECRET/channel",
        "https://example.com/%252574/EXAMPLE_SECRET/channel",
        UNSAFE,
        "https://example.com/channel#EXAMPLE_SECRET",
        "file:///channel?opaque=EXAMPLE_SECRET",
        "https://[EXAMPLE_SECRET",
    ] {
        let mut builder = LockFile::builder();
        builder.set_channels("EXAMPLE_SECRET", [value]);
        let lock = builder.finish();
        let before = lock.render_to_string().unwrap();
        assert!(before.contains("EXAMPLE_SECRET"));
        rejected(&lock, "environments[0].channels[0].url");
        assert_eq!(before, lock.render_to_string().unwrap());
    }
}

#[test]
fn public_urls_digest_fragments_and_local_paths_pass() {
    for value in [
        SAFE,
        "https://example.com/t/",
        "https://example.com/a#sha256:abcd",
        "https://example.com/a#md5:0123",
        "./channel",
        "/opt/channel",
        "C:\\channel",
        "file:///opt/channel",
    ] {
        let mut builder = LockFile::builder();
        builder.set_channels("default", [value]);
        builder.set_pypi_indexes(
            "default",
            PypiIndexes {
                indexes: vec![url("https://pypi.org/simple")],
                find_links: vec![FindLinksUrlOrPath::Path("./wheels".into())],
            },
        );
        let lock = builder.finish();
        let before = lock.render_to_string().unwrap();
        lock.validate_urls_for_export().unwrap();
        assert_eq!(before, lock.render_to_string().unwrap());
    }
    with_package(LockedPackage::Conda(conda().into()))
        .validate_urls_for_export()
        .unwrap();
}

#[test]
fn conda_package_locations_and_channels_are_checked() {
    let mut package = conda();
    package.location = url(UNSAFE).into();
    rejected(
        &with_package(LockedPackage::Conda(package.into())),
        "packages[0].location",
    );
    let mut package = conda();
    package.channel = Some(url(UNSAFE).into());
    rejected(
        &with_package(LockedPackage::Conda(package.into())),
        "packages[0].channel",
    );
}

#[test]
fn source_package_urls_are_checked_without_exposing_source_names() {
    let mut package = conda_source();
    package.location = url(UNSAFE).into();
    rejected(
        &with_package(LockedPackage::Conda(CondaPackageData::Source(Box::new(
            package,
        )))),
        "packages[0].location",
    );
    for source in [
        PackageBuildSource::Git {
            url: url(UNSAFE),
            spec: None,
            rev: "main".into(),
            subdir: None,
        },
        PackageBuildSource::Url {
            url: url(UNSAFE),
            sha256: rattler_digest::Sha256Hash::default(),
            subdir: None,
        },
    ] {
        let mut package = conda_source();
        package.package_build_source = Some(source);
        rejected(
            &with_package(LockedPackage::Conda(CondaPackageData::Source(Box::new(
                package,
            )))),
            "packages[0].package_build_source",
        );
    }
    for source in [
        SourceLocation::Url(UrlSourceLocation {
            url: url(UNSAFE),
            md5: None,
            sha256: None,
            subdirectory: None,
        }),
        SourceLocation::Git(GitSourceLocation {
            git: url(UNSAFE),
            rev: None,
            subdirectory: None,
            lfs: None,
        }),
    ] {
        let mut package = conda_source();
        package.sources.insert("EXAMPLE_SECRET".into(), source);
        rejected(
            &with_package(LockedPackage::Conda(CondaPackageData::Source(Box::new(
                package,
            )))),
            "packages[0].sources[0]",
        );
    }
    let mut package = conda_source();
    package.package_build_source = Some(PackageBuildSource::Path { path: ".".into() });
    package.sources.insert(
        "demo".into(),
        SourceLocation::Path(PathSourceLocation {
            path: "../demo".into(),
        }),
    );
    with_package(LockedPackage::Conda(CondaPackageData::Source(Box::new(
        package,
    ))))
    .validate_urls_for_export()
    .unwrap();
}

#[test]
fn pypi_locations_verbatim_spellings_indexes_and_requirements_are_checked() {
    let mut package = pypi();
    package.location = Verbatim::new(url(UNSAFE).into());
    rejected(
        &with_package(LockedPackage::Pypi(PypiPackageData::Distribution(
            Box::new(package),
        ))),
        "packages[0].location",
    );
    let mut package = pypi();
    package.location.set_given(UNSAFE.into());
    rejected(
        &with_package(LockedPackage::Pypi(PypiPackageData::Distribution(
            Box::new(package),
        ))),
        "packages[0].location.given",
    );
    let mut package = pypi();
    package.index_url = Some(url(UNSAFE));
    rejected(
        &with_package(LockedPackage::Pypi(PypiPackageData::Distribution(
            Box::new(package),
        ))),
        "packages[0].index_url",
    );
    let mut package = pypi();
    package
        .requires_dist
        .push(format!("dependency @ {UNSAFE}").parse().unwrap());
    rejected(
        &with_package(LockedPackage::Pypi(PypiPackageData::Distribution(
            Box::new(package),
        ))),
        "packages[0].requires_dist[0]",
    );
    let package = PypiSourceData {
        name: "demo".parse().unwrap(),
        location: Verbatim::new_with_given(UrlOrPath::Path(".".into()), UNSAFE.into()),
        requires_dist: vec![],
        requires_python: None,
        source_data: crate::SourceData::default(),
    };
    rejected(
        &with_package(LockedPackage::Pypi(PypiPackageData::Source(Box::new(
            package,
        )))),
        "packages[0].location.given",
    );
}

#[test]
fn environment_indexes_and_find_links_are_checked() {
    for (indexes, field) in [
        (
            PypiIndexes {
                indexes: vec![url(UNSAFE)],
                find_links: vec![],
            },
            "environments[0].indexes[0]",
        ),
        (
            PypiIndexes {
                indexes: vec![],
                find_links: vec![FindLinksUrlOrPath::Url(url(UNSAFE))],
            },
            "environments[0].find_links[0]",
        ),
    ] {
        let mut builder = LockFile::builder();
        builder.set_pypi_indexes("EXAMPLE_SECRET", indexes);
        rejected(&builder.finish(), field);
    }
}
