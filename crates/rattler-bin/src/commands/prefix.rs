use std::path::{Path, PathBuf};

use miette::{Context, IntoDiagnostic};
use rattler::install::Installer;
use rattler_conda_types::{
    MatchSpec, Matches, PackageName, PackageRecord, ParseStrictness, Platform, PrefixRecord,
    RepoDataRecord, package::DistArchiveIdentifier,
};
use url::Url;

const PIXI_ENVIRONMENT_FINGERPRINT_FILE: &str = ".pixi-environment-fingerprint";

/// Add one or more conda package archives to a prefix without solving.
#[derive(Debug, clap::Parser)]
pub struct InjectOpt {
    /// Local paths to conda package archives (.conda or .tar.bz2)
    #[clap(required = true)]
    packages: Vec<PathBuf>,

    /// Target prefix to inject the package into
    #[clap(short = 'p', long = "prefix", default_value = ".prefix")]
    target_prefix: PathBuf,

    /// Skip platform and dependency compatibility checks
    #[clap(long)]
    skip_compatibility_checks: bool,
}

/// Remove one or more installed conda packages from a prefix without solving.
#[derive(Debug, clap::Parser)]
pub struct RemoveFromPrefixOpt {
    /// Exact package names of installed packages to remove
    #[clap(required = true)]
    packages: Vec<PackageName>,

    /// Target prefix to remove the package from
    #[clap(short = 'p', long = "prefix", default_value = ".prefix")]
    target_prefix: PathBuf,

    /// Skip dependency compatibility checks after removal
    #[clap(long)]
    skip_compatibility_checks: bool,
}

pub async fn inject(opt: InjectOpt) -> miette::Result<()> {
    let target_prefix = std::path::absolute(opt.target_prefix).into_diagnostic()?;
    let packages = opt
        .packages
        .into_iter()
        .map(|package_path| {
            let package_path = package_path.canonicalize().into_diagnostic()?;
            let package_record = rattler_index::package_record_from_archive(&package_path)
                .into_diagnostic()
                .with_context(|| {
                    format!(
                        "failed to read package metadata from {}",
                        package_path.display()
                    )
                })?;

            if !opt.skip_compatibility_checks {
                validate_package_compatibility(&package_record)?;
            }

            Ok((package_path, package_record))
        })
        .collect::<miette::Result<Vec<_>>>()?;

    let installed_packages =
        PrefixRecord::collect_from_prefix::<PrefixRecord>(&target_prefix).into_diagnostic()?;

    for (_, package_record) in &packages {
        reject_already_installed(&installed_packages, &package_record.name)?;
    }
    reject_duplicate_package_records(packages.iter().map(|(_, package_record)| package_record))?;

    if !opt.skip_compatibility_checks {
        let records = installed_packages
            .iter()
            .map(|record| &record.repodata_record.package_record)
            .chain(packages.iter().map(|(_, package_record)| package_record))
            .collect::<Vec<_>>();
        PackageRecord::validate(records)
            .into_diagnostic()
            .context("injecting these packages would make the prefix incompatible")?;
    }

    let repodata_records = packages
        .into_iter()
        .map(|(package_path, package_record)| {
            let repodata_record = RepoDataRecord {
                package_record,
                identifier: DistArchiveIdentifier::try_from_path(&package_path).ok_or_else(
                    || {
                        miette::miette!(
                            "could not derive package identity from {}",
                            package_path.display()
                        )
                    },
                )?,
                url: Url::from_file_path(&package_path).map_err(|_err| {
                    miette::miette!("could not convert {} to a file URL", package_path.display())
                })?,
                channel: None,
            };

            Ok((package_path, repodata_record))
        })
        .collect::<miette::Result<Vec<_>>>()?;

    let mut desired_records = installed_packages
        .iter()
        .map(|record| record.repodata_record.clone())
        .collect::<Vec<_>>();
    let injected_package_records = repodata_records
        .iter()
        .map(|(_, repodata_record)| repodata_record.package_record.clone())
        .collect::<Vec<_>>();
    desired_records.extend(
        repodata_records
            .iter()
            .map(|(_, repodata_record)| repodata_record.clone()),
    );

    Installer::new()
        .with_target_platform(Platform::current())
        .with_installed_packages(installed_packages)
        .with_execute_link_scripts(true)
        .install(&target_prefix, desired_records)
        .await
        .into_diagnostic()
        .context("failed to inject packages into prefix")?;

    invalidate_pixi_environment_fingerprint(&target_prefix)?;

    for package_record in injected_package_records {
        println!(
            "{} Injected {} into {}",
            console::style(console::Emoji("✔", "")).green(),
            package_record,
            target_prefix.display()
        );
    }

    Ok(())
}

pub async fn remove_from_prefix(opt: RemoveFromPrefixOpt) -> miette::Result<()> {
    let target_prefix = std::path::absolute(opt.target_prefix).into_diagnostic()?;
    let installed_packages =
        PrefixRecord::collect_from_prefix::<PrefixRecord>(&target_prefix).into_diagnostic()?;
    reject_duplicate_package_names(&opt.packages)?;
    let remove_records = opt
        .packages
        .iter()
        .map(|package| select_installed_record(&installed_packages, package))
        .collect::<miette::Result<Vec<_>>>()?;
    let removed_package_records = remove_records
        .iter()
        .map(|record| record.repodata_record.package_record.clone())
        .collect::<Vec<_>>();
    let remaining_packages = installed_packages
        .iter()
        .filter(|record| !remove_records.contains(record))
        .cloned()
        .collect::<Vec<_>>();

    if !opt.skip_compatibility_checks {
        let records = remaining_packages
            .iter()
            .map(|record| &record.repodata_record.package_record)
            .collect::<Vec<_>>();
        PackageRecord::validate(records)
            .into_diagnostic()
            .context("removing these packages would make the prefix incompatible")?;
    }

    let desired_records = remaining_packages
        .iter()
        .map(|record| record.repodata_record.clone())
        .collect::<Vec<_>>();

    Installer::new()
        .with_target_platform(Platform::current())
        .with_installed_packages(installed_packages)
        .with_execute_link_scripts(true)
        .install(&target_prefix, desired_records)
        .await
        .into_diagnostic()
        .context("failed to remove packages from prefix")?;

    invalidate_pixi_environment_fingerprint(&target_prefix)?;

    for package_record in removed_package_records {
        println!(
            "{} Removed {} from {}",
            console::style(console::Emoji("✔", "")).green(),
            package_record,
            target_prefix.display()
        );
    }

    Ok(())
}

fn validate_package_compatibility(package_record: &PackageRecord) -> miette::Result<()> {
    validate_package_compatibility_for_platform(package_record, Platform::current())
}

fn validate_package_compatibility_for_platform(
    package_record: &PackageRecord,
    platform: Platform,
) -> miette::Result<()> {
    let package_subdir = &package_record.subdir;
    if package_subdir != &Platform::NoArch.to_string() && package_subdir != &platform.to_string() {
        return Err(miette::miette!(
            "package {} is for platform {}, but the current platform is {}",
            package_record,
            package_subdir,
            platform
        ));
    }

    validate_virtual_package_dependencies(package_record, platform)?;

    Ok(())
}

fn validate_virtual_package_dependencies(
    package_record: &PackageRecord,
    platform: Platform,
) -> miette::Result<()> {
    let virtual_packages = rattler_virtual_packages::VirtualPackages::detect_for_platform(
        platform,
        &rattler_virtual_packages::VirtualPackageOverrides::from_env(),
        rattler::default_cache_dir().ok().as_deref(),
    )
    .into_diagnostic()
    .with_context(|| format!("failed to determine virtual packages for {platform}"))?
    .into_generic_virtual_packages()
    .collect::<Vec<_>>();

    for dependency in &package_record.depends {
        let dependency_spec = MatchSpec::from_str(dependency, ParseStrictness::Lenient)
            .into_diagnostic()
            .with_context(|| format!("failed to parse dependency '{dependency}'"))?;
        if dependency_spec.is_virtual()
            && !virtual_packages
                .iter()
                .any(|virtual_package| dependency_spec.matches(virtual_package))
        {
            return Err(miette::miette!(
                "package {} has virtual dependency '{}', which is not satisfied by platform {}",
                package_record,
                dependency,
                platform
            ));
        }
    }

    Ok(())
}

fn reject_already_installed(
    installed_packages: &[PrefixRecord],
    package_name: &PackageName,
) -> miette::Result<()> {
    if installed_packages.iter().any(|record| {
        record.repodata_record.package_record.name.as_normalized() == package_name.as_normalized()
    }) {
        return Err(miette::miette!(
            "package {} is already installed",
            package_name.as_normalized()
        ));
    }

    Ok(())
}

fn reject_duplicate_package_records<'a>(
    package_records: impl IntoIterator<Item = &'a PackageRecord>,
) -> miette::Result<()> {
    let mut seen_package_names = Vec::<&PackageName>::new();

    for package_record in package_records {
        if seen_package_names
            .iter()
            .any(|package_name| package_name.as_normalized() == package_record.name.as_normalized())
        {
            return Err(miette::miette!(
                "package {} was specified multiple times",
                package_record.name.as_normalized()
            ));
        }

        seen_package_names.push(&package_record.name);
    }

    Ok(())
}

fn reject_duplicate_package_names(package_names: &[PackageName]) -> miette::Result<()> {
    let mut seen_package_names = Vec::<&PackageName>::new();

    for package_name in package_names {
        if seen_package_names.iter().any(|seen_package_name| {
            seen_package_name.as_normalized() == package_name.as_normalized()
        }) {
            return Err(miette::miette!(
                "package {} was specified multiple times",
                package_name.as_normalized()
            ));
        }

        seen_package_names.push(package_name);
    }

    Ok(())
}

fn invalidate_pixi_environment_fingerprint(target_prefix: &Path) -> miette::Result<()> {
    let fingerprint_path = target_prefix
        .join("conda-meta")
        .join(PIXI_ENVIRONMENT_FINGERPRINT_FILE);
    if fingerprint_path
        .try_exists()
        .into_diagnostic()
        .with_context(|| format!("failed to check {}", fingerprint_path.display()))?
    {
        std::fs::remove_file(&fingerprint_path)
            .into_diagnostic()
            .with_context(|| format!("failed to remove {}", fingerprint_path.display()))?;
    }

    Ok(())
}

fn select_installed_record<'a>(
    installed_packages: &'a [PrefixRecord],
    package_name: &PackageName,
) -> miette::Result<&'a PrefixRecord> {
    let matches = installed_packages
        .iter()
        .filter(|record| record.repodata_record.package_record.name == *package_name)
        .collect::<Vec<_>>();

    match matches.as_slice() {
        [] => Err(miette::miette!(
            "no installed package matched '{}'",
            package_name.as_normalized()
        )),
        [record] => Ok(*record),
        records => Err(miette::miette!(
            "package name '{}' matched multiple installed packages: {}",
            package_name.as_normalized(),
            records
                .iter()
                .map(|record| record.repodata_record.package_record.to_string())
                .collect::<Vec<_>>()
                .join(", ")
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{fs::File, str::FromStr};

    use rattler_conda_types::{Version, compression_level::CompressionLevel};
    use rattler_package_streaming::write::write_tar_bz2_package;

    #[tokio::test]
    async fn test_inject_and_remove_local_packages() {
        let prefix = tempfile::tempdir().unwrap();
        let fingerprint_path = prefix
            .path()
            .join("conda-meta")
            .join(PIXI_ENVIRONMENT_FINGERPRINT_FILE);
        std::fs::create_dir_all(fingerprint_path.parent().unwrap()).unwrap();
        std::fs::write(&fingerprint_path, "stale").unwrap();
        let package = workspace_root()
            .join("test-data")
            .join("packages")
            .join("empty-0.1.0-h4616a5c_0.conda");
        let other_package = write_empty_package(prefix.path(), "other-empty");

        inject(InjectOpt {
            packages: vec![package, other_package],
            target_prefix: prefix.path().to_path_buf(),
            skip_compatibility_checks: false,
        })
        .await
        .unwrap();

        let records = PrefixRecord::collect_from_prefix::<PrefixRecord>(prefix.path()).unwrap();
        assert_eq!(records.len(), 2);
        let package_names = records
            .iter()
            .map(|record| {
                record
                    .repodata_record
                    .package_record
                    .name
                    .as_normalized()
                    .to_string()
            })
            .collect::<Vec<_>>();
        assert!(package_names.iter().any(|name| name == "empty"));
        assert!(package_names.iter().any(|name| name == "other-empty"));
        assert!(!fingerprint_path.exists());

        std::fs::write(&fingerprint_path, "stale").unwrap();

        remove_from_prefix(RemoveFromPrefixOpt {
            packages: vec![
                PackageName::new_unchecked("empty"),
                PackageName::new_unchecked("other-empty"),
            ],
            target_prefix: prefix.path().to_path_buf(),
            skip_compatibility_checks: false,
        })
        .await
        .unwrap();

        let records = PrefixRecord::collect_from_prefix::<PrefixRecord>(prefix.path()).unwrap();
        assert!(records.is_empty());
        assert!(!fingerprint_path.exists());
    }

    #[tokio::test]
    async fn test_inject_same_name_twice_errors() {
        let prefix = tempfile::tempdir().unwrap();
        let package = workspace_root()
            .join("test-data")
            .join("packages")
            .join("empty-0.1.0-h4616a5c_0.conda");

        inject(InjectOpt {
            packages: vec![package.clone()],
            target_prefix: prefix.path().to_path_buf(),
            skip_compatibility_checks: false,
        })
        .await
        .unwrap();

        let result = inject(InjectOpt {
            packages: vec![package],
            target_prefix: prefix.path().to_path_buf(),
            skip_compatibility_checks: false,
        })
        .await;

        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("already installed")
        );
    }

    #[tokio::test]
    async fn test_inject_duplicate_requested_package_errors() {
        let prefix = tempfile::tempdir().unwrap();
        let package = workspace_root()
            .join("test-data")
            .join("packages")
            .join("empty-0.1.0-h4616a5c_0.conda");

        let result = inject(InjectOpt {
            packages: vec![package.clone(), package],
            target_prefix: prefix.path().to_path_buf(),
            skip_compatibility_checks: false,
        })
        .await;

        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("specified multiple times")
        );
    }

    #[tokio::test]
    async fn test_remove_duplicate_requested_package_errors() {
        let prefix = tempfile::tempdir().unwrap();
        let package = workspace_root()
            .join("test-data")
            .join("packages")
            .join("empty-0.1.0-h4616a5c_0.conda");

        inject(InjectOpt {
            packages: vec![package],
            target_prefix: prefix.path().to_path_buf(),
            skip_compatibility_checks: false,
        })
        .await
        .unwrap();

        let result = remove_from_prefix(RemoveFromPrefixOpt {
            packages: vec![
                PackageName::new_unchecked("empty"),
                PackageName::new_unchecked("empty"),
            ],
            target_prefix: prefix.path().to_path_buf(),
            skip_compatibility_checks: false,
        })
        .await;

        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("specified multiple times")
        );
    }

    #[test]
    fn test_virtual_package_dependencies_are_validated_for_target_platform() {
        let mut record = PackageRecord::new(
            PackageName::new_unchecked("win-only-noarch-package"),
            Version::from_str("0.0.1").unwrap(),
            "h123456".to_string(),
        );
        record.subdir = Platform::NoArch.to_string();
        record.depends = vec!["__win".to_string()];

        let err =
            validate_package_compatibility_for_platform(&record, Platform::OsxArm64).unwrap_err();
        assert!(err.to_string().contains("virtual dependency '__win'"));

        validate_package_compatibility_for_platform(&record, Platform::Win64).unwrap();
    }

    #[test]
    fn test_remove_selector_exact_name() {
        let record = PrefixRecord::from_repodata_record(
            RepoDataRecord {
                package_record: PackageRecord::new(
                    PackageName::new_unchecked("my-package"),
                    Version::from_str("0.0.1").unwrap(),
                    "h123456".to_string(),
                ),
                identifier: "my-package-0.0.1-h123456.conda".parse().unwrap(),
                url: Url::parse("https://example.com/my-package-0.0.1-h123456.conda").unwrap(),
                channel: None,
            },
            Vec::new(),
        );
        let installed = vec![record];

        assert!(
            select_installed_record(&installed, &PackageName::new_unchecked("my-package")).is_ok()
        );
        assert!(
            select_installed_record(&installed, &PackageName::new_unchecked("other-package"))
                .is_err()
        );
    }

    fn write_empty_package(root: &Path, package_name: &str) -> PathBuf {
        let package_build_dir = root.join(package_name);
        let package_info_dir = package_build_dir.join("info");
        std::fs::create_dir(&package_build_dir).unwrap();
        std::fs::create_dir(&package_info_dir).unwrap();
        std::fs::write(
            package_info_dir.join("index.json"),
            format!(
                r#"{{
                    "build": "h123456_0",
                    "build_number": 0,
                    "name": "{package_name}",
                    "noarch": "generic",
                    "subdir": "noarch",
                    "version": "0.1.0"
                }}"#
            ),
        )
        .unwrap();
        std::fs::write(
            package_info_dir.join("paths.json"),
            r#"{"paths":[],"paths_version":1}"#,
        )
        .unwrap();

        let target_package = root.join(format!("{package_name}-0.1.0-h123456_0.tar.bz2"));
        let writer = File::create(&target_package).unwrap();
        write_tar_bz2_package(
            writer,
            &package_build_dir,
            &[
                package_info_dir.join("index.json"),
                package_info_dir.join("paths.json"),
            ],
            CompressionLevel::Default,
            None,
            None,
        )
        .unwrap();

        target_package
    }

    fn workspace_root() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .parent()
            .unwrap()
            .to_path_buf()
    }
}
