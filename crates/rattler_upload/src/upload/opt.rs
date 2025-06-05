//! Command-line options.

use std::{ path::PathBuf };

use clap::{Parser, arg };
use rattler_solve::ChannelPriority;
use url::Url;
use rattler_conda_types::{NamedChannelOrUrl, Platform };

/// Container for rattler_solver::ChannelPriority so that it can be parsed
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct ChannelPriorityWrapper {
    /// The ChannelPriority value to be used when building the Configuration
    pub value: ChannelPriority,
}



/// Common opts that are shared between [`Rebuild`] and [`Build`]` subcommands
#[derive(Parser, Clone, Debug)]
pub struct CommonOpts {
    /// Output directory for build artifacts.
    #[clap(
        long,
        env = "CONDA_BLD_PATH",
        verbatim_doc_comment,
        help_heading = "Modifying result"
    )]
    pub output_dir: Option<PathBuf>,

    /// Enable support for repodata.json.zst
    #[clap(long, env = "RATTLER_ZSTD", default_value = "true", hide = true)]
    pub use_zstd: bool,

    /// Enable support for repodata.json.bz2
    #[clap(long, env = "RATTLER_BZ2", default_value = "true", hide = true)]
    pub use_bz2: bool,

    /// Enable experimental features
    #[arg(long, env = "RATTLER_BUILD_EXPERIMENTAL")]
    pub experimental: bool,

    /// List of hosts for which SSL certificate verification should be skipped
    #[arg(long, value_delimiter = ',')]
    pub allow_insecure_host: Option<Vec<String>>,

    /// Path to an auth-file to read authentication information from
    #[clap(long, env = "RATTLER_AUTH_FILE", hide = true)]
    pub auth_file: Option<PathBuf>,

    /// Channel priority to use when solving
    #[arg(long)]
    pub channel_priority: Option<ChannelPriorityWrapper>,
}



/// Upload options.
#[derive(Parser, Debug)]
pub struct UploadOpts {
    /// The package file to upload
    #[arg(global = true, required = false)]
    pub package_files: Vec<PathBuf>,

    /// The server type
    #[clap(subcommand)]
    pub server_type: ServerType,

    /// Common options.
    #[clap(flatten)]
    pub common: CommonOpts,
}

/// Server type.
#[derive(Clone, Debug, PartialEq, Parser)]
#[allow(missing_docs)]
pub enum ServerType {
    Quetz(QuetzOpts),
    Artifactory(ArtifactoryOpts),
    Prefix(PrefixOpts),
    Anaconda(AnacondaOpts),
    S3(S3Opts),
    #[clap(hide = true)]
    CondaForge(CondaForgeOpts),
}

/// Upload to a Quetz server.
/// Authentication is used from the keychain / auth-file.
#[derive(Clone, Debug, PartialEq, Parser)]
pub struct QuetzOpts {
    /// The URL to your Quetz server
    #[arg(short, long, env = "QUETZ_SERVER_URL")]
    pub url: Url,

    /// The URL to your channel
    #[arg(short, long = "channel", env = "QUETZ_CHANNEL")]
    pub channels: String,

    /// The Quetz API key, if none is provided, the token is read from the
    /// keychain / auth-file
    #[arg(short, long, env = "QUETZ_API_KEY")]
    pub api_key: Option<String>,
}

#[derive(Debug)]
#[allow(missing_docs)]
pub struct QuetzData {
    pub url: UrlWithTrailingSlash,
    pub channels: String,
    pub api_key: Option<String>,
}

impl From<QuetzOpts> for QuetzData {
    fn from(value: QuetzOpts) -> Self {
        Self::new(value.url, value.channels, value.api_key)
    }
}

impl QuetzData {
    /// Create a new instance of `QuetzData`
    pub fn new(url: Url, channels: String, api_key: Option<String>) -> Self {
        Self {
            url: url.into(),
            channels,
            api_key,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Parser)]
/// Options for uploading to a Artifactory channel.
/// Authentication is used from the keychain / auth-file.
pub struct ArtifactoryOpts {
    /// The URL to your Artifactory server
    #[arg(short, long, env = "ARTIFACTORY_SERVER_URL")]
    pub url: Url,

    /// The URL to your channel
    #[arg(short, long = "channel", env = "ARTIFACTORY_CHANNEL")]
    pub channels: String,

    /// Your Artifactory username
    #[arg(long, env = "ARTIFACTORY_USERNAME", hide = true)]
    pub username: Option<String>,

    /// Your Artifactory password
    #[arg(long, env = "ARTIFACTORY_PASSWORD", hide = true)]
    pub password: Option<String>,

    /// Your Artifactory token
    #[arg(short, long, env = "ARTIFACTORY_TOKEN")]
    pub token: Option<String>,
}

#[derive(Debug)]
#[allow(missing_docs)]
pub struct ArtifactoryData {
    pub url: UrlWithTrailingSlash,
    pub channels: String,
    pub token: Option<String>,
}

impl TryFrom<ArtifactoryOpts> for ArtifactoryData {
    type Error = miette::Error;

    fn try_from(value: ArtifactoryOpts) -> Result<Self, Self::Error> {
        let token = match (value.username, value.password, value.token) {
            (_, _, Some(token)) => Some(token),
            (Some(_), Some(password), _) => {
                warn!(
                    "Using username and password for Artifactory authentication is deprecated, using password as token. Please use an API token instead."
                );
                Some(password)
            }
            (Some(_), None, _) => {
                return Err(miette::miette!(
                    "Artifactory username provided without a password"
                ));
            }
            (None, Some(_), _) => {
                return Err(miette::miette!(
                    "Artifactory password provided without a username"
                ));
            }
            _ => None,
        };
        Ok(Self::new(value.url, value.channels, token))
    }
}

impl ArtifactoryData {
    /// Create a new instance of `ArtifactoryData`
    pub fn new(url: Url, channels: String, token: Option<String>) -> Self {
        Self {
            url: url.into(),
            channels,
            token,
        }
    }
}

/// Options for uploading to a prefix.dev server.
/// Authentication is used from the keychain / auth-file
#[derive(Clone, Debug, PartialEq, Parser)]
pub struct PrefixOpts {
    /// The URL to the prefix.dev server (only necessary for self-hosted
    /// instances)
    #[arg(
        short,
        long,
        env = "PREFIX_SERVER_URL",
        default_value = "https://prefix.dev"
    )]
    pub url: Url,

    /// The channel to upload the package to
    #[arg(short, long, env = "PREFIX_CHANNEL")]
    pub channel: String,

    /// The prefix.dev API key, if none is provided, the token is read from the
    /// keychain / auth-file
    #[arg(short, long, env = "PREFIX_API_KEY")]
    pub api_key: Option<String>,

    /// Upload one or more attestation files alongside the package
    /// Note: if you add an attestation, you can _only_ upload a single package.
    #[arg(long, required = false)]
    pub attestation: Option<PathBuf>,

    /// Skip upload if package is existed.
    #[arg(short, long)]
    pub skip_existing: bool,
}

#[derive(Debug)]
#[allow(missing_docs)]
pub struct PrefixData {
    pub url: UrlWithTrailingSlash,
    pub channel: String,
    pub api_key: Option<String>,
    pub attestation: Option<PathBuf>,
    pub skip_existing: bool,
}

impl From<PrefixOpts> for PrefixData {
    fn from(value: PrefixOpts) -> Self {
        Self::new(
            value.url,
            value.channel,
            value.api_key,
            value.attestation,
            value.skip_existing,
        )
    }
}

impl PrefixData {
    /// Create a new instance of `PrefixData`
    pub fn new(
        url: Url,
        channel: String,
        api_key: Option<String>,
        attestation: Option<PathBuf>,
        skip_existing: bool,
    ) -> Self {
        Self {
            url: url.into(),
            channel,
            api_key,
            attestation,
            skip_existing,
        }
    }
}

/// Options for uploading to a Anaconda.org server
#[derive(Clone, Debug, PartialEq, Parser)]
pub struct AnacondaOpts {
    /// The owner of the distribution (e.g. conda-forge or your username)
    #[arg(short, long, env = "ANACONDA_OWNER")]
    pub owner: String,

    /// The channel / label to upload the package to (e.g. main / rc)
    #[arg(short, long = "channel", env = "ANACONDA_CHANNEL")]
    pub channels: Option<Vec<String>>,

    /// The Anaconda API key, if none is provided, the token is read from the
    /// keychain / auth-file
    #[arg(short, long, env = "ANACONDA_API_KEY")]
    pub api_key: Option<String>,

    /// The URL to the Anaconda server
    #[arg(short, long, env = "ANACONDA_SERVER_URL")]
    pub url: Option<Url>,

    /// Replace files on conflict
    #[arg(long, short, env = "ANACONDA_FORCE")]
    pub force: bool,
}

fn parse_s3_url(value: &str) -> Result<Url, String> {
    let url: Url = Url::parse(value).map_err(|_| format!("`{}` isn't a valid URL", value))?;
    if url.scheme() == "s3" && url.host_str().is_some() {
        Ok(url)
    } else {
        Err(format!(
            "Only S3 URLs of format s3://bucket/... can be used, not `{}`",
            value
        ))
    }
}

/// Options for uploading to S3
#[derive(Clone, Debug, PartialEq, Parser)]
pub struct S3Opts {
    /// The channel URL in the S3 bucket to upload the package to, e.g., s3://my-bucket/my-channel
    #[arg(short, long, env = "S3_CHANNEL", value_parser = parse_s3_url)]
    pub channel: Url,

    /// The endpoint URL of the S3 backend
    #[arg(
        long,
        env = "S3_ENDPOINT_URL",
        default_value = "https://s3.amazonaws.com"
    )]
    pub endpoint_url: Url,

    /// The region of the S3 backend
    #[arg(long, env = "S3_REGION", default_value = "eu-central-1")]
    pub region: String,

    /// Whether to use path-style S3 URLs
    #[arg(long, env = "S3_FORCE_PATH_STYLE", default_value = "false")]
    pub force_path_style: bool,

    /// The access key ID for the S3 bucket.
    #[arg(long, env = "S3_ACCESS_KEY_ID", requires_all = ["secret_access_key"])]
    pub access_key_id: Option<String>,

    /// The secret access key for the S3 bucket.
    #[arg(long, env = "S3_SECRET_ACCESS_KEY", requires_all = ["access_key_id"])]
    pub secret_access_key: Option<String>,

    /// The session token for the S3 bucket.
    #[arg(long, env = "S3_SESSION_TOKEN", requires_all = ["access_key_id", "secret_access_key"])]
    pub session_token: Option<String>,
}

#[derive(Debug)]
#[allow(missing_docs)]
pub struct AnacondaData {
    pub owner: String,
    pub channels: Vec<String>,
    pub api_key: Option<String>,
    pub url: UrlWithTrailingSlash,
    pub force: bool,
}

impl From<AnacondaOpts> for AnacondaData {
    fn from(value: AnacondaOpts) -> Self {
        Self::new(
            value.owner,
            value.channels,
            value.api_key,
            value.url,
            value.force,
        )
    }
}

impl AnacondaData {
    /// Create a new instance of `PrefixData`
    pub fn new(
        owner: String,
        channel: Option<Vec<String>>,
        api_key: Option<String>,
        url: Option<Url>,
        force: bool,
    ) -> Self {
        Self {
            owner,
            channels: channel.unwrap_or_else(|| vec!["main".to_string()]),
            api_key,
            url: url
                .unwrap_or_else(|| Url::parse("https://api.anaconda.org").unwrap())
                .into(),
            force,
        }
    }
}

/// Options for uploading to conda-forge
#[derive(Clone, Debug, PartialEq, Parser)]
pub struct CondaForgeOpts {
    /// The Anaconda API key
    #[arg(long, env = "STAGING_BINSTAR_TOKEN")]
    pub staging_token: String,

    /// The feedstock name
    #[arg(long, env = "FEEDSTOCK_NAME")]
    pub feedstock: String,

    /// The feedstock token
    #[arg(long, env = "FEEDSTOCK_TOKEN")]
    pub feedstock_token: String,

    /// The staging channel name
    #[arg(long, env = "STAGING_CHANNEL")]
    pub staging_channel: Option<String>,

    /// The Anaconda Server URL
    #[arg(long, env = "ANACONDA_SERVER_URL")]
    pub anaconda_url: Option<Url>,

    /// The validation endpoint url
    #[arg(long, env = "VALIDATION_ENDPOINT")]
    pub validation_endpoint: Option<Url>,

    /// The CI provider
    #[arg(long, env = "CI")]
    pub provider: Option<String>,

    /// Dry run, don't actually upload anything
    #[arg(long, env = "DRY_RUN")]
    pub dry_run: bool,
}

#[derive(Debug)]
#[allow(missing_docs)]
pub struct CondaForgeData {
    pub staging_token: String,
    pub feedstock: String,
    pub feedstock_token: String,
    pub staging_channel: String,
    pub anaconda_url: UrlWithTrailingSlash,
    pub validation_endpoint: Url,
    pub provider: Option<String>,
    pub dry_run: bool,
}

impl From<CondaForgeOpts> for CondaForgeData {
    fn from(value: CondaForgeOpts) -> Self {
        Self::new(
            value.staging_token,
            value.feedstock,
            value.feedstock_token,
            value.staging_channel,
            value.anaconda_url,
            value.validation_endpoint,
            value.provider,
            value.dry_run,
        )
    }
}

impl CondaForgeData {
    /// Create a new instance of `PrefixData`
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        staging_token: String,
        feedstock: String,
        feedstock_token: String,
        staging_channel: Option<String>,
        anaconda_url: Option<Url>,
        validation_endpoint: Option<Url>,
        provider: Option<String>,
        dry_run: bool,
    ) -> Self {
        Self {
            staging_token,
            feedstock,
            feedstock_token,
            staging_channel: staging_channel.unwrap_or_else(|| "cf-staging".to_string()),
            anaconda_url: anaconda_url
                .unwrap_or_else(|| Url::parse("https://api.anaconda.org").unwrap())
                .into(),
            validation_endpoint: validation_endpoint.unwrap_or_else(|| {
                Url::parse("https://conda-forge.herokuapp.com/feedstock-outputs/copy").unwrap()
            }),
            provider,
            dry_run,
        }
    }
}

/// Debug options
#[derive(Parser)]
pub struct DebugOpts {
    /// Recipe file to debug
    #[arg(short, long)]
    pub recipe: PathBuf,

    /// Output directory for build artifacts
    #[arg(short, long)]
    pub output: Option<PathBuf>,

    /// The target platform to build for
    #[arg(long)]
    pub target_platform: Option<Platform>,

    /// The host platform to build for (defaults to target_platform)
    #[arg(long)]
    pub host_platform: Option<Platform>,

    /// The build platform to build for (defaults to current platform)
    #[arg(long)]
    pub build_platform: Option<Platform>,

    /// Channels to use when building
    #[arg(short = 'c', long = "channel")]
    pub channels: Option<Vec<NamedChannelOrUrl>>,

    /// Common options
    #[clap(flatten)]
    pub common: CommonOpts,

    /// Name of the specific output to debug (only required when a recipe has multiple outputs)
    #[arg(long, help = "Name of the specific output to debug")]
    pub output_name: Option<String>,
}

#[derive(Debug, Clone)]
/// Data structure containing the configuration for debugging a recipe
pub struct DebugData {
    /// Path to the recipe file to debug
    pub recipe_path: PathBuf,
    /// Directory where build artifacts will be stored
    pub output_dir: PathBuf,
    /// Platform where the build is being executed
    pub build_platform: Platform,
    /// Target platform for the build
    pub target_platform: Platform,
    /// Host platform for runtime dependencies
    pub host_platform: Platform,
    /// List of channels to search for dependencies
    pub channels: Option<Vec<NamedChannelOrUrl>>,
    /// Common configuration options
    pub common: CommonData,
    /// Name of the specific output to debug (if recipe has multiple outputs)
    pub output_name: Option<String>,
}

impl DebugData {
    /// Generate a new TestData struct from TestOpts and an optional pixi config.
    /// TestOpts have higher priority than the pixi config.
    pub fn from_opts_and_config(opts: DebugOpts, config: Option<pixi_config::Config>) -> Self {
        Self {
            recipe_path: opts.recipe,
            output_dir: opts.output.unwrap_or_else(|| PathBuf::from("./output")),
            build_platform: opts.build_platform.unwrap_or(Platform::current()),
            target_platform: opts.target_platform.unwrap_or(Platform::current()),
            host_platform: opts
                .host_platform
                .unwrap_or_else(|| opts.target_platform.unwrap_or(Platform::current())),
            channels: opts.channels,
            common: CommonData::from_opts_and_config(opts.common, config.unwrap_or_default()),
            output_name: opts.output_name,
        }
    }
}
