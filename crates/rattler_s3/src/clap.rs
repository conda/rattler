use crate::S3Credentials;
use clap::Parser;
use url::Url;

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
}

impl From<S3CredentialsOpts> for Option<S3Credentials> {
    fn from(value: S3CredentialsOpts) -> Self {
        if let (Some(endpoint_url), Some(region)) = (value.endpoint_url, value.region) {
            Some(S3Credentials {
                endpoint_url,
                region,
                access_key_id: value.access_key_id,
                secret_access_key: value.secret_access_key,
                session_token: value.session_token,
            })
        } else {
            None
        }
    }
}
