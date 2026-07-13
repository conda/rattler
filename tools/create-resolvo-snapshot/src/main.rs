use std::{collections::HashSet, io::BufWriter, path::Path};

use clap::Parser;
use itertools::Itertools;
use rattler_conda_types::{Channel, ChannelConfig, MatchSpec, Platform};
use rattler_repodata_gateway::Gateway;
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

    // Determine the subdirs to query.
    let mut subdirs: HashSet<Platform> = HashSet::from_iter(args.subdir);
    if subdirs.is_empty() {
        subdirs.insert(Platform::current());
    }
    subdirs.insert(Platform::NoArch);
    let platforms = subdirs.iter().copied().collect_vec();

    // Construct a gateway to fetch repodata. The gateway transparently handles
    // sharded repodata which is required to capture channels like `conda-pypi`
    // that do not expose a monolithic `repodata.json`.
    let gateway = Gateway::builder()
        .with_cache_dir(
            rattler_cache::default_cache_dir()
                .unwrap()
                .join(rattler_cache::REPODATA_CACHE_DIR),
        )
        .finish();

    // Enumerate every package name available in the channel so we can pull in
    // the complete set of records, not just those reachable from a single spec.
    eprintln!("fetching package names..");
    let names = gateway
        .names(vec![channel.clone()], platforms.clone())
        .await
        .unwrap();
    eprintln!("found {} package names", names.len());

    // Query the repodata for all names. Providing every name guarantees that the
    // resulting snapshot captures the entire channel.
    eprintln!("fetching repodata..");
    let specs = names.into_iter().map(MatchSpec::from).collect_vec();
    let repodatas = gateway
        .query(vec![channel.clone()], platforms, specs)
        .recursive(false)
        .await
        .unwrap();

    let total_records: usize = repodatas
        .iter()
        .map(rattler_repodata_gateway::RepoData::len)
        .sum();
    eprintln!("fetched {total_records} records");

    // Create the dependency provider
    let provider = rattler_solve::resolvo::CondaDependencyProvider::new(
        repodatas.iter().map(|repo_data| {
            repo_data
                .iter()
                .collect::<rattler_solve::resolvo::RepoData<'_>>()
        }),
        &[],
        &[],
        &[],
        &[],
        None,
        None,
        ChannelPriority::default(),
        None,
        SolveStrategy::default(),
        Vec::new(),
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
