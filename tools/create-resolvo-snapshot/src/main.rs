use std::{collections::HashSet, io::BufWriter, path::Path};

use clap::Parser;
use itertools::Itertools;
use rattler_conda_types::{Channel, ChannelConfig, Platform};
use rattler_networking::LazyClient;
use rattler_repodata_gateway::fetch::FetchRepoDataOptions;
use rattler_solve::{ChannelPriority, SolveStrategy};

#[derive(Parser)]
#[clap(about)]
struct Args {
    /// The channel to make the snapshot for.
    channel: String,

    /// The subdirs to query.
    #[clap(short, long, num_args=1..)]
    subdir: Vec<Platform>,

    /// The output path
    #[clap(short)]
    output: Option<String>,
}

#[tokio::main]
async fn main() {
    let args: Args = Args::parse();

    // Determine the channel
    let channel = Channel::from_str(
        &args.channel,
        &ChannelConfig::default_with_root_dir(std::env::current_dir().unwrap()),
    )
    .unwrap();

    // Fetch the repodata for all the subdirs.
    let mut subdirs: HashSet<Platform> = HashSet::from_iter(args.subdir);
    if subdirs.is_empty() {
        subdirs.insert(Platform::current());
    }
    subdirs.insert(Platform::NoArch);

    let client = LazyClient::default();
    let mut records = Vec::new();
    for &subdir in &subdirs {
        eprintln!("fetching repodata for {subdir:?}..");
        let repodata = rattler_repodata_gateway::fetch::fetch_repo_data(
            channel.platform_url(subdir),
            client.clone(),
            rattler_cache::default_cache_dir()
                .unwrap()
                .join(rattler_cache::REPODATA_CACHE_DIR),
            FetchRepoDataOptions::default(),
            None,
        )
        .await
        .unwrap();

        eprintln!("parsing repodata..");
        let repodata = rattler_conda_types::RepoData::from_path(repodata.repo_data_json_path)
            .unwrap()
            .into_repo_data_records(&channel);

        records.push(repodata);
    }

    // Create the dependency provider
    let provider = rattler_solve::resolvo::CondaDependencyProvider::new(
        records
            .iter()
            .map(rattler_solve::resolvo::RepoData::from_iter),
        &[],
        &[],
        &[],
        &[],
        None,
        ChannelPriority::default(),
        None,
        SolveStrategy::default(),
    )
    .unwrap();

    eprintln!("creating snapshot..");
    let package_names = provider.package_names().collect::<Vec<_>>();
    let snapshot =
        resolvo::snapshot::DependencySnapshot::from_provider(provider, package_names, [], [])
            .unwrap();

    let output_file = args.output.unwrap_or_else(|| {
        format!(
            "snapshot-{}-{}.json",
            channel.name(),
            subdirs
                .iter()
                .copied()
                .map(Platform::as_str)
                .sorted()
                .join("-")
        )
    });
    eprintln!("serializing snapshot to {}", &output_file);
    let snapshot_path = Path::new(&output_file);
    if let Some(dir) = snapshot_path.parent() {
        std::fs::create_dir_all(dir).unwrap();
    }
    let snapshot_file = BufWriter::new(std::fs::File::create(snapshot_path).unwrap());
    serde_json::to_writer(snapshot_file, &snapshot).unwrap();
}
