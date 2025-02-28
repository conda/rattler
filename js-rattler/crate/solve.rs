use std::{path::PathBuf, str::FromStr};

use rattler_conda_types::{Channel, ChannelConfig, MatchSpec, ParseStrictness::Lenient, Platform};
use rattler_repodata_gateway::{Gateway, SourceConfig};
use rattler_solve::{SolverImpl, SolverTask};
use wasm_bindgen::prelude::*;

use crate::error::JsError;

/// A package that has been solved by the solver.
/// @public
#[wasm_bindgen(getter_with_clone)]
pub struct SolvedPackage {
    pub url: String,
    pub package_name: String,
    pub build_number: u64,
    pub repo_name: Option<String>,
    pub filename: String,
    pub version: String,
}

/// Solve a set of specs with the given channels and platforms.
/// @public
#[wasm_bindgen]
pub async fn simple_solve(
    specs: Vec<String>,
    channels: Vec<String>,
    platforms: Vec<String>,
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
        .map(|p| Platform::from_str(&p))
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

    // Solve
    let task = SolverTask {
        specs,
        ..repodata.iter().collect::<SolverTask<_>>()
    };
    let solved = rattler_solve::resolvo::Solver.solve(task)?;

    // Convert to JS types
    Ok(solved
        .records
        .into_iter()
        .map(|r| SolvedPackage {
            url: r.url.to_string(),
            package_name: r.package_record.name.as_source().to_string(),
            build_number: r.package_record.build_number,
            repo_name: r.channel,
            filename: r.file_name,
            version: r.package_record.version.to_string(),
        })
        .collect())
}
