//! Command-line options.
use std::path::PathBuf;

use clap::{arg, Parser};
use rattler_conda_types::utils::url_with_trailing_slash::UrlWithTrailingSlash;
use rattler_networking::AuthenticationStorage;
use tracing::warn;
use url::Url;

/// Newtype wrapper for the force overwrite flag.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ForceOverwrite(pub bool);

impl ForceOverwrite {
    /// Returns `true` if force overwrite is enabled.
    pub fn is_enabled(&self) -> bool {
        self.0
    }
}

impl From<bool> for ForceOverwrite {
    fn from(value: bool) -> Self {
        Self(value)
    }
}

/// Newtype wrapper for the skip existing flag.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SkipExisting(pub bool);

impl SkipExisting {
    /// Returns `true` if skip existing is enabled.
    pub fn is_enabled(&self) -> bool {
        self.0
    }
}

impl From<bool> for SkipExisting {
    fn from(value: bool) -> Self {
        Self(value)
    }
}

/// Source for attestation when uploading packages.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum AttestationSource {
    /// Provide an existing attestation file path.
    Attestation(PathBuf),
    /// Automatically generate attestation using cosign on CI.
    GenerateAttestation,
    /// No attestation.
    #[default]
    NoAttestation,
}

/// Common opts for upload operations
#[derive(Parser, Clone, Debug, Default)]
pub struct CommonOpts {
    /// List of hosts for which SSL certificate verification should be skipped
    #[arg(long, value_delimiter = ',')]
    pub allow_insecure_host: Option<Vec<String>>,

    /// Path to an auth-file to read authentication information from
    #[clap(long, env = "RATTLER_AUTH_FILE", hide = true)]
    pub auth_file: Option<PathBuf>,
}

#[derive(Clone, Debug)]
#[allow(missing_docs)]
pub struct CommonData {
    pub auth_file: Option<PathBuf>,
    pub allow_insecure_host: Option<Vec<String>>,
}

impl CommonData {
    /// Create a new instance of `CommonData`
    pub fn new(auth_file: Option<PathBuf>, allow_insecure_host: Option<Vec<String>>) -> Self {
        Self {
            auth_file,
            allow_insecure_host,
        }
    }

    /// Create from `CommonOpts`
    pub fn from_opts(value: CommonOpts) -> Self {
        Self::new(value.auth_file, value.allow_insecure_host)
    }
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

    #[clap(skip)]
    pub auth_store: Option<AuthenticationStorage>,
}

impl UploadOpts {
    pub fn with_auth_store(mut self, auth_store: Option<AuthenticationStorage>) -> Self {
        self.auth_store = auth_store;
        self
    }
}

/// Server type.
#[derive(Clone, Debug, PartialEq, Parser)]
#[allow(missing_docs)]
pub enum ServerType {
    Quetz(QuetzOpts),
    Artifactory(ArtifactoryOpts),
    Prefix(PrefixOpts),
    Anaconda(AnacondaOpts),
    #[cfg(feature = "s3")]
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

    /// Upload an attestation file alongside the package.
    /// Note: if you add an attestation, you can _only_ upload a single package.
    /// Mutually exclusive with --generate-attestation.
    #[arg(long, conflicts_with = "generate_attestation")]
    pub attestation: Option<PathBuf>,

    /// Automatically generate attestation using cosign in CI.
    /// Mutually exclusive with --attestation.
    #[arg(long, conflicts_with = "attestation")]
    pub generate_attestation: bool,

    /// Also store the generated attestation to GitHub's attestation API.
    /// Requires `GITHUB_TOKEN` environment variable and only works in GitHub Actions.
    /// The attestation will be associated with the current repository.
    #[arg(long, requires = "generate_attestation")]
    pub store_github_attestation: bool,

    /// Skip upload if package already exists.
    #[arg(short, long)]
    pub skip_existing: bool,

    /// Force overwrite existing packages
    #[arg(long)]
    pub force: bool,
}

#[derive(Debug)]
#[allow(missing_docs)]
pub struct PrefixData {
    pub url: UrlWithTrailingSlash,
    pub channel: String,
    pub api_key: Option<String>,
    pub attestation: AttestationSource,
    pub skip_existing: SkipExisting,
    pub force: ForceOverwrite,
    pub store_github_attestation: bool,
}

impl From<PrefixOpts> for PrefixData {
    fn from(value: PrefixOpts) -> Self {
        let attestation = match (value.attestation, value.generate_attestation) {
            (Some(path), false) => AttestationSource::Attestation(path),
            (None, true) => AttestationSource::GenerateAttestation,
            _ => AttestationSource::NoAttestation,
        };
        Self::new(
            value.url,
            value.channel,
            value.api_key,
            attestation,
            value.skip_existing.into(),
            value.force.into(),
            value.store_github_attestation,
        )
    }
}

impl PrefixData {
    /// Create a new instance of `PrefixData`
    pub fn new(
        url: Url,
        channel: String,
        api_key: Option<String>,
        attestation: AttestationSource,
        skip_existing: SkipExisting,
        force: ForceOverwrite,
        store_github_attestation: bool,
    ) -> Self {
        Self {
            url: url.into(),
            channel,
            api_key,
            attestation,
            skip_existing,
            force,
            store_github_attestation,
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

#[cfg(feature = "s3")]
fn parse_s3_url(value: &str) -> Result<Url, String> {
    let url: Url =
        Url::parse(value).map_err(|err| format!("`{value}` isn't a valid URL: {err}"))?;
    if url.scheme() == "s3" && url.host_str().is_some() {
        Ok(url)
    } else {
        Err(format!(
            "Only S3 URLs of format s3://bucket/... can be used, not `{value}`"
        ))
    }
}

/// Options for uploading to S3
#[cfg(feature = "s3")]
#[derive(Clone, Debug, PartialEq, Parser)]
pub struct S3Opts {
    /// The channel URL in the S3 bucket to upload the package to, e.g.,
    /// `s3://my-bucket/my-channel`
    #[arg(short, long, env = "S3_CHANNEL", value_parser = parse_s3_url)]
    pub channel: Url,

    #[clap(flatten)]
    pub credentials: rattler_s3::clap::S3CredentialsOpts,

    /// Replace files if it already exists.
    #[arg(long)]
    pub force: bool,
}

#[derive(Debug)]
#[allow(missing_docs)]
pub struct AnacondaData {
    pub owner: String,
    pub channels: Vec<String>,
    pub api_key: Option<String>,
    pub url: UrlWithTrailingSlash,
    pub force: ForceOverwrite,
}

impl From<AnacondaOpts> for AnacondaData {
    fn from(value: AnacondaOpts) -> Self {
        Self::new(
            value.owner,
            value.channels,
            value.api_key,
            value.url,
            value.force.into(),
        )
    }
}

impl AnacondaData {
    /// Create a new instance of `AnacondaData`
    pub fn new(
        owner: String,
        channel: Option<Vec<String>>,
        api_key: Option<String>,
        url: Option<Url>,
        force: ForceOverwrite,
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
