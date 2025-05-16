use std::path::PathBuf;
use std::str::FromStr;

use rattler_conda_types::{
    ParseChannelError, Channel, ChannelConfig, MatchSpec, ParseStrictness::Lenient, PackageName, Version, RepoDataRecord, PackageRecord, NoArchType,
};
use rattler_repodata_gateway::{Gateway, SourceConfig};
use rattler_solve::{SolverImpl, SolverTask};
use wasm_bindgen::prelude::*;

use crate::{error::JsError, platform::JsPlatform};

use web_sys::console::log_1;
use serde::{Serialize, Deserialize};

use std::collections::BTreeMap;
use chrono::{DateTime, Utc};

use url::Url;


#[wasm_bindgen(getter_with_clone)]
#[derive(Serialize)]
pub struct SolvedPackage {
    pub url: String,
    #[wasm_bindgen(js_name = "packageName")]
    pub package_name: String,
    pub build: String,
    #[wasm_bindgen(js_name = "buildNumber")]
    pub build_number: u64,
    #[wasm_bindgen(js_name = "repoName")]
    pub repo_name: Option<String>,
    pub filename: String,
    pub version: String,
}


#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct JsLockedPackage {
    url: String,
    package_name: String,
    build: String,
    build_number: u64,
    repo_name: Option<String>,
    filename: String,
    version: String,
    subdir: String,
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
        .map(|s| MatchSpec::from_str(&s, Lenient))
        .collect::<Result<Vec<_>, _>>()?;
    let channels = channels
        .into_iter()
        .map(|s| Channel::from_str(&s, &channel_config))
        .collect::<Result<Vec<_>, _>>()?;
    let platforms = platforms
        .into_iter()
        .map(|p| serde_wasm_bindgen::from_value(p.into()))
        .collect::<Result<Vec<_>, _>>()?;

    // Fetch the repodata
    let gateway = Gateway::builder()
        .with_channel_config(rattler_repodata_gateway::ChannelConfig {
            default: SourceConfig {
                sharded_enabled: true,
                ..SourceConfig::default()
            },
            ..rattler_repodata_gateway::ChannelConfig::default()
        })
        .finish();

    let repodata = gateway
        .query(channels, platforms, specs.iter().cloned())
        .recursive(true)
        .execute()
        .await?;

    let js_locked_packages: Vec<JsLockedPackage> =
        serde_wasm_bindgen::from_value(locked_packages).map_err(JsError::from)?;

    let locked_packages = js_locked_packages
        .into_iter()
        .map(|pkg| {
            let url = Url::parse(&pkg.url).map_err(|e| JsError::from(ParseChannelError::from(e)))?;


        let package_record = PackageRecord {
                name: PackageName::try_from(pkg.package_name.clone())?,
                version: Version::from_str(&pkg.version)?.into(),
                build:  pkg.build.clone(),
                build_number: pkg.build_number,
                subdir: "unknown".to_string(),
                md5: None,
                sha256: None,
                size: None,
                arch: None,
                platform: None,
                depends: vec![],
                extra_depends: std::collections::BTreeMap::new(),
                constrains: vec![],
                track_features: vec![],
                features: None,
                noarch:  NoArchType::none(),
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
                file_name: pkg.filename,
                channel: pkg.repo_name,
                package_record,
            })
        })
        .collect::<Result<Vec<_>, JsError>>()?;

    let task = SolverTask {
        specs,
        locked_packages,
        pinned_packages: vec![],
        ..repodata.iter().collect::<SolverTask<_>>()
    };

    let solved = rattler_solve::resolvo::Solver.solve(task)?;

    let js_value = serde_wasm_bindgen::to_value(&solved.records).unwrap();
    log_1(&js_value);

    Ok(solved
        .records
        .into_iter()
        .map(|r| SolvedPackage {
            url: r.url.to_string(),
            package_name: r.package_record.name.as_source().to_string(),
            build: r.package_record.build.clone(),
            build_number: r.package_record.build_number,
            repo_name: r.channel,
            filename: r.file_name,
            version: r.package_record.version.to_string(),
        })
        .collect())
}
