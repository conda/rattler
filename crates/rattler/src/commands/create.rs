use itertools::Itertools;
use pubgrub::{
    error::PubGrubError,
    report::{DefaultStringReporter, Reporter},
    solver::resolve,
};
use std::str::FromStr;

use rattler::{
    repo_data::fetch::{terminal_progress, MultiRequestRepoDataBuilder},
    Channel, ChannelConfig, PackageIndex, PackageRecord, SolverIndex, Version,
};

#[derive(Debug, clap::Parser)]
pub struct Opt {
    #[clap(short)]
    channels: Option<Vec<String>>,

    #[clap(required = true)]
    specs: Vec<String>,
}

pub async fn create(opt: Opt) -> anyhow::Result<()> {
    let channel_config = ChannelConfig::default();

    // Get the cache directory
    let cache_dir = dirs::cache_dir()
        .ok_or_else(|| anyhow::anyhow!("could not determine cache directory for current platform"))?
        .join("rattler/cache");
    std::fs::create_dir_all(&cache_dir)
        .map_err(|e| anyhow::anyhow!("could not create cache directory: {}", e))?;

    // Get the channels to download
    let channels = opt
        .channels
        .unwrap_or_else(|| vec![String::from("conda-forge")])
        .into_iter()
        .map(|channel_str| Channel::from_str(&channel_str, &channel_config))
        .collect::<Result<Vec<_>, _>>()?;

    // Download all repo data from the channels and create an index
    let repo_data_per_source = MultiRequestRepoDataBuilder::default()
        .set_cache_dir(&cache_dir)
        .set_listener(terminal_progress())
        .set_fail_fast(false)
        .add_channels(channels)
        .request()
        .await;

    // Error out if fetching one of the sources resulted in an error.
    let repo_data = repo_data_per_source
        .into_iter()
        .map(|(_, _, result)| result)
        .collect::<Result<Vec<_>, _>>()?;

    let index = PackageIndex::from(repo_data);

    let mut solve_index = SolverIndex::new(index);

    let root_package_name = String::from("__solver");
    let root_version = Version::from_str("1").unwrap();
    let root_package = PackageRecord {
        name: root_package_name.clone(),
        version: root_version,
        build: "".to_string(),
        build_number: 0,
        subdir: "".to_string(),
        md5: None,
        sha256: None,
        arch: None,
        platform: None,
        depends: opt.specs,
        constrains: vec![],
        track_features: vec![],
        features: None,
        noarch: None,
        preferred_env: None,
        license: None,
        license_family: None,
        timestamp: None,
        date: None,
        size: None,
    };

    solve_index.add(root_package.clone());

    match resolve(&solve_index, root_package_name, root_package) {
        Ok(result) => {
            let pinned_packages: Vec<_> = result.into_iter().collect();
            let longest_package_name = pinned_packages
                .iter()
                .map(|(package_name, _)| package_name.len())
                .max()
                .unwrap_or(0);

            println!("Found a solution!");
            for (package, version) in pinned_packages.iter().sorted_by_key(|(package, _)| package) {
                println!(
                    "- {:<longest_package_name$} {}",
                    package,
                    version,
                    longest_package_name = longest_package_name
                )
            }
        }
        Err(PubGrubError::NoSolution(mut derivation_tree)) => {
            derivation_tree.collapse_no_versions();
            eprintln!(
                "Could not find a solution:\n{}",
                DefaultStringReporter::report(&derivation_tree)
            );
        }
        Err(e) => eprintln!("could not find a solution!\n{}", e),
    }

    Ok(())
}
