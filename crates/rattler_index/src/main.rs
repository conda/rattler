use std::path::PathBuf;

#[cfg(feature = "s3")]
use anyhow::Context;
use clap::{Parser, Subcommand};
use clap_verbosity_flag::Verbosity;
#[cfg(feature = "azure")]
use rattler_azure::{AzureChannelUrl, AzureEndpoint, AzureHost};
use rattler_conda_types::Platform;
use rattler_config::config::{
    concurrency::default_max_concurrent_solves, index::IndexChannelConfig,
};
#[cfg(feature = "s3")]
use rattler_index::PreconditionChecks;
use rattler_index::{
    ChannelMetadata, IndexFsConfig, PackageRevisionAssignment, index_fs_with_channel_metadata,
};
#[cfg(feature = "azure")]
use rattler_index::{IndexAzureConfig, index_azure_with_channel_metadata};
#[cfg(feature = "s3")]
use rattler_index::{IndexS3Config, index_s3_with_channel_metadata};
#[cfg(feature = "s3")]
use rattler_networking::AuthenticationStorage;
#[cfg(feature = "s3")]
use rattler_s3::S3Credentials;
#[cfg(feature = "s3")]
use url::Url;

#[cfg(feature = "s3")]
fn parse_s3_url(value: &str) -> Result<Url, String> {
    let url: Url = Url::parse(value).map_err(|e| format!("`{value}` isn't a valid URL: {e}"))?;
    if url.scheme() == "s3" && url.host_str().is_some() {
        Ok(url)
    } else {
        Err(format!(
            "Only S3 URLs of format s3://bucket/... can be used, not `{value}`"
        ))
    }
}

/// SAS permissions requested when minting a user-delegation SAS for indexing.
/// Indexing does a read-modify-write of repodata and lists/reads packages, so it
/// needs read, write, list, and create (`r` + `w` + `l` + `c`).
#[cfg(feature = "azure")]
const AZURE_INDEX_SAS_PERMISSIONS: &str = "rwlc";

/// The `rattler-index` CLI.
#[derive(Parser)]
#[command(name = "rattler-index", version, about, long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Commands,

    #[command(flatten)]
    verbosity: Verbosity,

    /// Whether to force the re-indexing of all packages.
    /// Note that this will create a new repodata.json instead of updating the
    /// existing one.
    #[arg(short, long, default_value = "false", global = true)]
    force: bool,

    /// The maximum number of packages to process in-memory simultaneously.
    /// This is necessary to limit memory usage when indexing large channels.
    #[arg(long, global = true)]
    max_parallel: Option<usize>,

    /// A specific platform to index.
    /// Defaults to all platforms available in the channel.
    #[arg(long, global = true)]
    target_platform: Option<Platform>,

    /// The name of the conda package (expected to be in the `noarch` subdir)
    /// that should be used for repodata patching. For more information, see `https://prefix.dev/blog/repodata_patching`.
    #[arg(long, global = true)]
    repodata_patch: Option<String>,

    /// Disable precondition checks (`ETags`, timestamps) during file operations.
    /// Use this flag if your S3 backend doesn't fully support conditional requests,
    /// or if you're certain no concurrent indexing processes are running.
    /// Warning: Disabling this removes protection against concurrent modifications.
    #[cfg(feature = "s3")]
    #[arg(long, default_value = "false", global = true)]
    disable_precondition_checks: bool,

    /// The path to the config file to use to configure rattler-index.
    /// Uses the same configuration format as pixi, see `https://pixi.sh/latest/reference/pixi_configuration`.
    /// Per-channel index options are read from the `index-config` section.
    #[arg(long)]
    config: Option<PathBuf>,
}

/// The subcommands for the `rattler-index` CLI.
#[derive(Subcommand)]
#[allow(clippy::large_enum_variant)]
enum Commands {
    /// Index a channel stored on the filesystem.
    #[command(name = "fs")]
    FileSystem {
        /// The path to the channel directory.
        #[arg()]
        channel: std::path::PathBuf,
    },

    /// Index a channel stored in an S3 bucket.
    #[cfg(feature = "s3")]
    S3 {
        /// The S3 channel URL, e.g. `s3://my-bucket/my-channel`.
        #[arg(value_parser = parse_s3_url)]
        channel: Url,

        #[clap(flatten)]
        credentials: rattler_s3::clap::S3CredentialsOpts,
    },

    /// Index a channel stored in an Azure Blob container.
    #[cfg(feature = "azure")]
    #[command(name = "az")]
    Azblob {
        /// The Azure Blob channel URL, e.g.
        /// `az://<account>.blob.core.windows.net/<container>/<channel>`.
        ///
        /// Parsed into an [`AzureChannelUrl`] rather than a wire `Url`: the wire
        /// scheme comes from the host's `azure-options` entry, which is not read
        /// until after clap has run, and the `az://` spelling is what
        /// `[index-config."…"]` keys are matched against.
        channel: AzureChannelUrl,

        #[clap(flatten)]
        credentials: rattler_azure::clap::AzureCredentialsOpts,
    },
}

/// The configuration type for rattler-index - just extends rattler config and
/// can load the same TOML files as pixi.
pub type Config = rattler_config::config::ConfigBase;

/// Entry point of the `rattler-index` cli.
#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Parse the command line arguments
    let cli = Cli::parse();

    tracing_subscriber::FmtSubscriber::builder()
        .with_max_level(cli.verbosity)
        .init();

    let multi_progress = indicatif::MultiProgress::new();

    let config = if let Some(config_path) = cli.config {
        Some(Config::load_from_files(vec![config_path])?)
    } else {
        None
    };
    let max_parallel = cli
        .max_parallel
        .or(config.as_ref().map(|c| c.concurrency.downloads))
        .unwrap_or_else(default_max_concurrent_solves);

    #[cfg(feature = "s3")]
    let precondition_checks = if cli.disable_precondition_checks {
        PreconditionChecks::Disabled
    } else {
        PreconditionChecks::Enabled
    };

    match cli.command {
        Commands::FileSystem { channel } => {
            let target = channel
                .canonicalize()
                .unwrap_or_else(|_| channel.clone())
                .to_string_lossy()
                .into_owned();
            let resolved = resolve_index_channel_config(&config, &target);
            let (write_zst, write_shards, repodata_revisions, package_revision_assignment) =
                effective_index_options(&resolved);
            let channel_metadata = ChannelMetadata::from_index_config(&resolved);

            index_fs_with_channel_metadata(
                IndexFsConfig {
                    channel,
                    target_platform: cli.target_platform,
                    repodata_patch: cli.repodata_patch,
                    write_zst,
                    write_shards,
                    repodata_revisions,
                    package_revision_assignment,
                    force: cli.force,
                    max_parallel,
                    multi_progress: Some(multi_progress),
                },
                channel_metadata,
            )
            .await
        }
        #[cfg(feature = "s3")]
        Commands::S3 {
            channel,
            mut credentials,
        } => {
            let target = channel.to_string();
            let resolved = resolve_index_channel_config(&config, &target);
            let (write_zst, write_shards, repodata_revisions, package_revision_assignment) =
                effective_index_options(&resolved);
            let channel_metadata = ChannelMetadata::from_index_config(&resolved);

            let bucket = channel.host().context("Invalid S3 url")?.to_string();
            let s3_config = config
                .as_ref()
                .and_then(|config| config.s3_options.0.get(&bucket));

            // Fill in missing credentials from config file if not provided on command line
            credentials.region = credentials.region.or(s3_config.map(|c| c.region.clone()));
            credentials.endpoint_url = credentials
                .endpoint_url
                .or(s3_config.map(|c| c.endpoint_url.clone()));

            // Resolve the credentials
            let credentials = match Option::<S3Credentials>::from(credentials) {
                Some(credentials) => {
                    let auth_storage = AuthenticationStorage::from_env_and_defaults()?;
                    credentials.resolve(&channel, &auth_storage).ok_or_else(|| anyhow::anyhow!("Could not find S3 credentials in the authentication storage, and no credentials were provided via the command line."))?
                }
                None => rattler_s3::ResolvedS3Credentials::from_sdk().await?,
            };

            index_s3_with_channel_metadata(
                IndexS3Config {
                    channel,
                    credentials,
                    target_platform: cli.target_platform,
                    repodata_patch: cli.repodata_patch,
                    write_zst,
                    write_shards,
                    repodata_revisions,
                    package_revision_assignment,
                    force: cli.force,
                    max_parallel,
                    multi_progress: Some(multi_progress),
                    precondition_checks,
                },
                channel_metadata,
            )
            .await
        }
        #[cfg(feature = "azure")]
        Commands::Azblob {
            channel,
            credentials,
        } => {
            // `canonical()`, not the wire URL: `[index-config."az://…"]` is how a
            // user keys an Azure channel, and matching the https spelling meant
            // such a key never applied to anything.
            let target = channel.canonical().to_string();
            let resolved = resolve_index_channel_config(&config, &target);
            let (write_zst, write_shards, repodata_revisions, package_revision_assignment) =
                effective_index_options(&resolved);
            let channel_metadata = ChannelMetadata::from_index_config(&resolved);

            let endpoint = azure_endpoint(&config, channel.host());

            let credentials = credentials
                .resolve(AZURE_INDEX_SAS_PERMISSIONS, &channel, endpoint)
                .await?;

            index_azure_with_channel_metadata(
                IndexAzureConfig {
                    channel,
                    credentials,
                    endpoint,
                    target_platform: cli.target_platform,
                    repodata_patch: cli.repodata_patch,
                    write_zst,
                    write_shards,
                    repodata_revisions,
                    package_revision_assignment,
                    force: cli.force,
                    max_parallel,
                    multi_progress: Some(multi_progress),
                },
                channel_metadata,
            )
            .await
        }
    }?;
    println!("Finished indexing channel.");
    Ok(())
}

/// How to address a channel's host, from its `[azure-options."<host>"]` entry, or
/// the https host-style defaults when there is no config file or no entry.
///
/// A host without an entry and a host with an empty entry are defined to behave
/// identically, so this never has to report which of the two it found. The entry's
/// per-container grants are not part of the result: indexing signs with the
/// credential its caller supplied, so there is no ambient chain for a grant to
/// gate.
#[cfg(feature = "azure")]
fn azure_endpoint(config: &Option<Config>, host: &AzureHost) -> AzureEndpoint {
    config
        .as_ref()
        .map(|config| config.azure_options.get(host).endpoint())
        .unwrap_or_default()
}

fn resolve_index_channel_config(config: &Option<Config>, target: &str) -> IndexChannelConfig {
    config
        .as_ref()
        .map(|c| c.index_config.resolve(target))
        .unwrap_or_default()
}

fn effective_index_options(
    cfg: &IndexChannelConfig,
) -> (
    bool,
    bool,
    Vec<rattler_index::RepodataRevisionInfo>,
    PackageRevisionAssignment,
) {
    let write_zst = cfg.write_zst.unwrap_or(true);
    let write_shards = cfg.write_shards.unwrap_or(true);
    let repodata_revisions = cfg.repodata_revisions.clone().unwrap_or_default();
    let package_revision_assignment = cfg.package_revision_assignment.unwrap_or_default();
    (
        write_zst,
        write_shards,
        repodata_revisions,
        package_revision_assignment,
    )
}

#[cfg(all(test, feature = "azure"))]
mod tests {
    use rattler_azure::{Addressing, AzureCredentials, AzureScheme};

    use super::*;

    /// Load a config from TOML the way `--config` does, through a real file, so
    /// the test exercises the same deserialization the CLI does.
    fn config_from(toml: &str) -> Option<Config> {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("rattler-config.toml");
        std::fs::write(&path, toml).expect("write config");
        Some(Config::load_from_files(vec![path]).expect("config should load"))
    }

    /// Reviewer issue 5: `[index-config."az://…"]` is the only spelling a user
    /// would write for an Azure channel, and matching the https wire URL meant it
    /// never applied to anything.
    #[test]
    fn index_config_is_keyed_by_the_canonical_az_url() {
        let config = config_from(
            r#"
            [index-config."az://acct.blob.core.windows.net/general"]
            write-shards = false
            "#,
        );
        let channel =
            AzureChannelUrl::parse("az://acct.blob.core.windows.net/general/mychannel").unwrap();

        let resolved = resolve_index_channel_config(&config, channel.canonical().as_str());
        assert_eq!(resolved.write_shards, Some(false));

        // The spelling this used to match against, kept as the negative half of
        // the proof: had the key been written in wire form it would still be dead.
        let wire = channel.wire(AzureScheme::Https).to_string();
        assert_eq!(
            resolve_index_channel_config(&config, &wire).write_shards,
            None
        );
    }

    /// An Azurite entry has to carry all the way to the opendal config, because
    /// every one of these four fields is derived differently under path-style and
    /// a wrong one fails silently.
    #[test]
    fn a_path_style_entry_drives_the_azurite_index_config() {
        let config = config_from(
            r#"
            [azure-options."127.0.0.1:10000"]
            scheme = "http"
            path-style = true

            [azure-options."127.0.0.1:10000".auth]
            general = true
            "#,
        );
        let channel =
            AzureChannelUrl::parse("az://127.0.0.1:10000/devstoreaccount1/general/mychannel")
                .unwrap();

        let endpoint = azure_endpoint(&config, channel.host());
        assert_eq!(endpoint.scheme, AzureScheme::Http);
        assert_eq!(endpoint.addressing, Addressing::PathStyle);

        let azblob = rattler_azure::azblob_config(
            &AzureCredentials::AccountKey("key".into()),
            &channel,
            endpoint,
        )
        .expect("an Azurite channel must build an opendal config");

        assert_eq!(
            azblob.endpoint.as_deref(),
            Some("http://127.0.0.1:10000/devstoreaccount1")
        );
        assert_eq!(azblob.account_name.as_deref(), Some("devstoreaccount1"));
        assert_eq!(azblob.container, "general");
        assert_eq!(azblob.root.as_deref(), Some("/mychannel"));
    }

    /// Without an entry the same URL is an error, not a silently different
    /// endpoint: host-style cannot read an account out of an IP literal, and the
    /// error names the config line that fixes it.
    #[test]
    fn an_emulator_host_without_an_entry_is_a_guided_error() {
        let channel =
            AzureChannelUrl::parse("az://127.0.0.1:10000/devstoreaccount1/general").unwrap();
        let endpoint = azure_endpoint(&None, channel.host());

        let err = rattler_azure::azblob_config(
            &AzureCredentials::AccountKey("key".into()),
            &channel,
            endpoint,
        )
        .expect_err("host-style cannot address an IP literal");
        let message = err.to_string();
        assert!(
            message.contains("[azure-options.\"127.0.0.1:10000\"]"),
            "{message}"
        );
        assert!(message.contains("path-style = true"), "{message}");
    }
}
