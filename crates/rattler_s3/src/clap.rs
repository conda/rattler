use clap::{Parser, ValueEnum};
use std::fmt::Display;
use url::Url;

use crate::S3Credentials;

#[derive(ValueEnum, Clone, Default, Debug, PartialEq)]
#[clap(rename_all = "kebab_case")]
pub enum S3AddressingStyleOpts {
    #[default]
    VirtualHost,
    Path,
}

impl From<S3AddressingStyleOpts> for crate::S3AddressingStyle {
    fn from(value: S3AddressingStyleOpts) -> Self {
        match value {
            S3AddressingStyleOpts::VirtualHost => crate::S3AddressingStyle::VirtualHost,
            S3AddressingStyleOpts::Path => crate::S3AddressingStyle::Path,
        }
    }
}

impl Display for S3AddressingStyleOpts {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            S3AddressingStyleOpts::VirtualHost => write!(f, "virtual-host"),
            S3AddressingStyleOpts::Path => write!(f, "path"),
        }
    }
}

/// Manually specified S3 credentials, when these are used no credentials are
/// read through the AWS SDK.
///
/// See [`super::S3Credentials`] for details on how these credentials are used.
#[derive(Clone, Debug, PartialEq, Parser)]
pub struct S3CredentialsOpts {
    /// The endpoint URL of the S3 backend
    #[arg(long, env = "S3_ENDPOINT_URL", requires_all = ["region"], help_heading = "S3 Credentials")]
    pub endpoint_url: Option<Url>,

    /// The region of the S3 backend
    #[arg(long, env = "S3_REGION", requires_all = ["endpoint_url"], help_heading = "S3 Credentials")]
    pub region: Option<String>,

    /// The access key ID for the S3 bucket.
    #[arg(long, env = "S3_ACCESS_KEY_ID", requires_all = ["secret_access_key", "endpoint_url", "region"], help_heading = "S3 Credentials")]
    pub access_key_id: Option<String>,

    /// The secret access key for the S3 bucket.
    #[arg(long, env = "S3_SECRET_ACCESS_KEY", requires_all = ["access_key_id", "endpoint_url", "region"], help_heading = "S3 Credentials")]
    pub secret_access_key: Option<String>,

    /// The session token for the S3 bucket.
    #[arg(long, env = "S3_SESSION_TOKEN", requires_all = ["access_key_id", "secret_access_key", "endpoint_url", "region"], help_heading = "S3 Credentials")]
    pub session_token: Option<String>,

    /// How to address the bucket
    #[arg(long, env = "S3_ADDRESSING_STYLE", requires_all = ["region", "endpoint_url"], help_heading = "S3 Credentials", conflicts_with="force_path_style", default_value_t=S3AddressingStyleOpts::default())]
    pub addressing_style: S3AddressingStyleOpts,

    /// Whether to use path-style S3 URLs
    #[arg(long, env = "S3_FORCE_PATH_STYLE", requires_all = ["region", "endpoint_url"], help_heading = "S3 Credentials", conflicts_with="addressing_style", hide = true, help = "[deprecated] Whether to use path-style S3 URLs")]
    pub force_path_style: Option<bool>,
}

impl From<S3CredentialsOpts> for Option<S3Credentials> {
    fn from(mut value: S3CredentialsOpts) -> Self {
        if value.force_path_style.is_some() {
            tracing::warn!("The `--force-path-style` option is deprecated, please use `--addressing-style=path` instead.");
            value.addressing_style = S3AddressingStyleOpts::Path;
        }
        if let (Some(endpoint_url), Some(region)) = (value.endpoint_url, Some(value.region)) {
            Some(S3Credentials {
                endpoint_url,
                region,
                access_key_id: value.access_key_id,
                secret_access_key: value.secret_access_key,
                session_token: value.session_token,
                addressing_style: value.addressing_style.into(),
            })
        } else {
            None
        }
    }
}
