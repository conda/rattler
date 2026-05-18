use std::path::PathBuf;
use std::str::FromStr;

use crate::{error::JsError, platform::JsPlatform};
use rattler_conda_types::{
    Channel, ChannelConfig, Flag, MatchSpec, NoArchType, PackageName, PackageRecord,
    ParseChannelError, ParseMatchSpecOptions, RepoDataRecord, RepodataRevision, Version,
    package::{
        ArchiveIdentifier, BuildString, CondaArchiveType, DistArchiveIdentifier, DistArchiveType,
    },
};
use rattler_digest::{Md5, Sha256, parse_digest_from_hex};
use rattler_repodata_gateway::{Gateway, SourceConfig};
use rattler_solve::{SolverImpl, SolverTask};
use reqwest::Client;
use reqwest_middleware::ClientWithMiddleware;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::collections::HashMap;
use url::Url;
use wasm_bindgen::prelude::*;

#[wasm_bindgen(getter_with_clone)]
#[derive(Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct SolvedPackage {
    pub url: String,
    #[wasm_bindgen(js_name = "packageName")]
    pub package_name: String,
    pub build: String,
    #[wasm_bindgen(js_name = "buildNumber")]
    pub build_number: Option<u64>,
    #[wasm_bindgen(js_name = "repoName")]
    pub repo_name: Option<String>,
    pub size: Option<u64>,
    pub filename: String,
    pub md5: Option<String>,
    pub sha256: Option<String>,
    pub version: String,
    pub depends: Option<Vec<String>>,
    extra_depends: Option<BTreeMap<String, Vec<String>>>,
    pub subdir: Option<String>,
    pub flags: Option<Vec<String>>,
}

#[wasm_bindgen]
impl SolvedPackage {
    /// Conditional or optional dependencies keyed by extra name.
    #[wasm_bindgen(
        getter,
        js_name = "extraDepends",
        unchecked_return_type = "Record<string, string[]> | undefined"
    )]
    pub fn extra_depends(&self) -> Result<JsValue, JsError> {
        let Some(extra_depends) = &self.extra_depends else {
            return Ok(JsValue::UNDEFINED);
        };
        let serializer = serde_wasm_bindgen::Serializer::json_compatible();
        Ok(extra_depends.serialize(&serializer)?)
    }
}

/// Solve a set of specs with the given channels and platforms.
/// @public
#[wasm_bindgen(js_name = "simpleSolve")]
pub async fn simple_solve(
    #[wasm_bindgen(param_description = "Matchspecs of packages that must be included.")] specs: Vec<
        String,
    >,
    #[wasm_bindgen(param_description = "The channels to request for repodata of packages")]
    channels: Vec<String>,
    #[wasm_bindgen(param_description = "The platforms to solve for")] platforms: Vec<JsPlatform>,
    #[wasm_bindgen(param_description = "Installed packages")] locked_packages: JsValue,
) -> Result<Vec<SolvedPackage>, JsError> {
    // TODO: Dont hardcode
    let channel_config = ChannelConfig::default_with_root_dir(PathBuf::from(""));

    // Convert types
    let specs = specs
        .into_iter()
        .map(|s| {
            MatchSpec::from_str(
                &s,
                ParseMatchSpecOptions::lenient().with_repodata_revision(RepodataRevision::V3),
            )
        })
        .collect::<Result<Vec<_>, _>>()?;
    let channels = channels
        .into_iter()
        .map(|s| Channel::from_str(&s, &channel_config))
        .collect::<Result<Vec<_>, _>>()?;
    let platforms = platforms
        .into_iter()
        .map(|p| serde_wasm_bindgen::from_value(p.into()))
        .collect::<Result<Vec<_>, _>>()?;
    let js_locked_packages: Vec<SolvedPackage> =
        if locked_packages.is_null() || locked_packages.is_undefined() {
            vec![]
        } else {
            serde_wasm_bindgen::from_value(locked_packages).map_err(JsError::from)?
        };
    let mut installed_packages: Vec<RepoDataRecord> = js_locked_packages
        .clone()
        .into_iter()
        .map(|pkg| {
            let url =
                Url::parse(&pkg.url).map_err(|e| JsError::from(ParseChannelError::from(e)))?;

            let rec = PackageRecord {
                name: PackageName::try_from(pkg.package_name.clone())?,
                version: Version::from_str(&pkg.version)?.into(),
                build: BuildString::new(pkg.build.clone()).map_err(JsError::from)?,
                build_number: pkg.build_number.unwrap_or_default(),
                md5: pkg
                    .md5
                    .as_ref()
                    .and_then(|s| parse_digest_from_hex::<Md5>(s)),
                sha256: pkg
                    .sha256
                    .as_ref()
                    .and_then(|s| parse_digest_from_hex::<Sha256>(s)),
                size: pkg.size,
                arch: None,
                platform: None,
                depends: pkg.depends.unwrap_or_default(),
                subdir: pkg.subdir.unwrap_or_else(|| "unknown".to_string()),
                extra_depends: pkg.extra_depends.unwrap_or_default(),
                constrains: vec![],
                track_features: vec![],
                features: None,
                flags: pkg
                    .flags
                    .unwrap_or_default()
                    .into_iter()
                    .map(Flag::new_unchecked)
                    .collect(),
                noarch: NoArchType::none(),
                license: None,
                license_family: None,
                timestamp: None,
                python_site_packages_path: None,
                legacy_bz2_md5: None,
                legacy_bz2_size: None,
                purls: None,
                run_exports: None,
            };

            Ok(RepoDataRecord {
                url,
                identifier: pkg.filename.parse().unwrap_or_else(|_| {
                    let archive_type = DistArchiveType::try_from(&pkg.filename)
                        .unwrap_or(CondaArchiveType::Conda.into());
                    DistArchiveIdentifier {
                        identifier: ArchiveIdentifier {
                            name: rec.name.as_source().to_string(),
                            version: rec.version.to_string(),
                            build_string: rec
                                .build
                                .as_ref()
                                .map_or_else(String::new, BuildString::to_string),
                        },
                        archive_type,
                    }
                }),
                channel: pkg.repo_name,
                package_record: rec.clone(),
            })
        })
        .collect::<Result<Vec<_>, JsError>>()?;

    // Fetch the repodata
    let gateway = Gateway::builder()
        // Creating the Gateway with a default client to avoid adding a user-agent header
        // (Not supported from the browser)
        .with_client(ClientWithMiddleware::from(Client::new()))
        .with_channel_config(rattler_repodata_gateway::ChannelConfig {
            default: SourceConfig {
                sharded_enabled: true,
                ..SourceConfig::default()
            },
            ..rattler_repodata_gateway::ChannelConfig::default()
        })
        .finish();

    let output = gateway
        .query(channels, platforms, specs.iter().cloned())
        .recursive(true)
        .execute()
        .await?;
    crate::gateway::emit_gateway_warnings(output.warnings);
    let repodata = output.repodata;

    // Use matching repodata metadata when older locked packages omit dependencies.
    // The URL identifies the exact artifact, avoiding metadata from an identically named
    // package in another channel or subdirectory.
    let repodata_metadata: HashMap<&Url, &PackageRecord> = repodata
        .iter()
        .flat_map(|r| r.iter())
        .map(|record| (&record.url, &record.package_record))
        .collect();

    // If a locked package omits dependencies, restore them from repodata.
    for records in installed_packages.iter_mut() {
        if let Some(metadata) = repodata_metadata.get(&records.url) {
            if records.package_record.depends.is_empty() {
                records.package_record.depends = metadata.depends.clone();
            }
            if records.package_record.extra_depends.is_empty() {
                records.package_record.extra_depends = metadata.extra_depends.clone();
            }
        }
    }

    let task = SolverTask {
        specs,
        locked_packages: installed_packages.iter().collect(),
        ..repodata.iter().collect::<SolverTask<'_, _>>()
    };

    let solved = rattler_solve::resolvo::Solver.solve(task)?;

    Ok(solved
        .records
        .into_iter()
        .map(|r| SolvedPackage {
            url: r.url.to_string(),
            package_name: r.package_record.name.as_source().to_string(),
            build: r
                .package_record
                .build
                .as_ref()
                .map_or_else(String::new, BuildString::to_string),
            build_number: Some(r.package_record.build_number),
            repo_name: r.channel,
            filename: r.identifier.to_file_name(),
            version: r.package_record.version.to_string(),
            md5: r.package_record.md5.as_ref().map(hex::encode),
            sha256: r.package_record.sha256.as_ref().map(hex::encode),
            size: r.package_record.size,
            depends: Some(r.package_record.depends.clone()),
            extra_depends: Some(r.package_record.extra_depends.clone()),
            subdir: Some(r.package_record.subdir.clone()),
            flags: Some(
                r.package_record
                    .flags
                    .iter()
                    .map(ToString::to_string)
                    .collect(),
            ),
        })
        .collect())
}
