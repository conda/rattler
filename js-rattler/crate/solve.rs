use std::path::PathBuf;

use rattler_conda_types::{
    Channel, ChannelConfig, ChannelUrl, MatchSpec, ParseStrictness::Lenient,
};
use rattler_repodata_gateway::{Gateway, SourceConfig};
use rattler_solve::{SolverImpl, SolverTask};
use wasm_bindgen::prelude::*;

use crate::{error::JsError, platform::JsPlatform};

/// A package that has been solved by the solver.
/// @public
#[wasm_bindgen(getter_with_clone)]
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
            build: r.package_record.build.clone(),
            build_number: r.package_record.build_number,
            repo_name: r.channel.as_ref().map(ChannelUrl::to_string),
            filename: r.file_name,
            version: r.package_record.version.to_string(),
        })
        .collect())
}
