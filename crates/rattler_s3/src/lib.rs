#[cfg(feature = "clap")]
pub mod clap;
#[cfg(feature = "opendal")]
mod signer;

use aws_config::{BehaviorVersion, Region};
use aws_credential_types::provider::{SharedCredentialsProvider, error::CredentialsError};
use aws_sdk_s3::config::{Credentials, ProvideCredentials};
use rattler_networking::{Authentication, AuthenticationStorage};
use url::Url;

pub use rattler_networking::s3_middleware::S3AddressingStyle;

/// Rattler based crates always either use S3 credentials specified by the user
/// through CLI arguments combined with credentials coming from `rattler auth`,
/// or everything is loaded through the AWS SDK.
#[derive(Debug, Clone)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct S3Credentials {
    /// The endpoint URL of the S3 backend
    pub endpoint_url: Url,

    /// The region of the S3 backend
    pub region: String,

    /// The addressing style to use for the bucket.
    #[cfg_attr(feature = "serde", serde(default))]
    pub addressing_style: S3AddressingStyle,

    /// The access key ID for the S3 bucket.
    pub access_key_id: Option<String>,

    /// The secret access key for the S3 bucket.
    pub secret_access_key: Option<String>,

    /// The session token for the S3 bucket.
    pub session_token: Option<String>,
}

/// The resolved S3 credentials.
#[derive(Debug, Clone)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct ResolvedS3Credentials {
    /// The endpoint URL of the S3 backend
    pub endpoint_url: Url,

    /// The region of the S3 backend
    pub region: String,

    /// How to address the S3 buckets.
    pub addressing_style: S3AddressingStyle,

    /// The access key ID for the S3 bucket.
    pub access_key_id: String,

    /// The secret access key for the S3 bucket.
    pub secret_access_key: String,

    /// The session token for the S3 bucket.
    pub session_token: Option<String>,
}

#[derive(Debug, thiserror::Error)]
pub enum FromSDKError {
    #[error("No credentials provider found in AWS SDK configuration")]
    NoCredentialsProvider,

    #[error("Could not determine region from AWS SDK configuration")]
    MissingRegion,

    #[error("Could not determine endpoint from AWS SDK configuration")]
    MissingEndpoint,

    #[error("Failed to parse endpoint from AWS SDK configuration")]
    InvalidEndpoint(#[source] url::ParseError),

    #[error(transparent)]
    CredentialsError(CredentialsError),
}

impl ResolvedS3Credentials {
    /// Resolve the settings and credentials of an S3 bucket through the AWS SDK.
    ///
    /// The credentials are resolved once. Prefer
    /// [`S3CredentialSource::from_sdk`] for anything long-running, so that
    /// temporary credentials can be refreshed when they expire.
    pub async fn from_sdk() -> Result<Self, FromSDKError> {
        S3CredentialSource::from_sdk()
            .await?
            .credentials()
            .await
            .map_err(FromSDKError::CredentialsError)
    }
}

/// The settings of an S3 bucket together with a provider for its credentials.
///
/// Where [`ResolvedS3Credentials`] holds a single fixed set of credentials, this
/// keeps the provider that produced them. That distinction matters for temporary
/// credentials — from AWS SSO, an assumed role or the instance metadata service
/// — because those expire after a couple of hours: an operation that runs longer
/// than that has to ask for a new set, and only the provider can hand one out.
///
/// With the `opendal` feature, [`Self::opendal_builder`] creates an operator
/// that does exactly that whenever the credentials it holds are about to expire.
#[derive(Clone, Debug)]
pub struct S3CredentialSource {
    /// The endpoint URL of the S3 backend
    pub endpoint_url: Url,

    /// The region of the S3 backend
    pub region: String,

    /// How to address the S3 buckets.
    pub addressing_style: S3AddressingStyle,

    /// The provider of the credentials to sign requests with.
    credentials_provider: SharedCredentialsProvider,
}

#[derive(Debug, thiserror::Error)]
pub enum ResolveError {
    #[error(
        "could not find S3 credentials for {0} in the authentication storage, and no credentials \
         were provided explicitly"
    )]
    MissingCredentials(Url),

    #[error(transparent)]
    FromSDK(#[from] FromSDKError),
}

impl S3CredentialSource {
    /// Determine how requests to the bucket of `bucket_url` should be signed.
    ///
    /// If `credentials` is given, its access keys are used, or the ones in
    /// `auth_storage` if it carries none itself. If it is not given, everything
    /// is resolved through the AWS SDK, which covers environment variables,
    /// `~/.aws/config` profiles (including SSO), `credential_process`, assumed
    /// roles, web identities and instance metadata.
    pub async fn resolve(
        credentials: Option<S3Credentials>,
        bucket_url: &Url,
        auth_storage: &AuthenticationStorage,
    ) -> Result<Self, ResolveError> {
        match credentials {
            Some(credentials) => credentials
                .resolve(bucket_url, auth_storage)
                .map(Self::from)
                .ok_or_else(|| ResolveError::MissingCredentials(bucket_url.clone())),
            None => Ok(Self::from_sdk().await?),
        }
    }

    /// Resolve the settings and the credential provider of an S3 bucket through
    /// the AWS SDK.
    pub async fn from_sdk() -> Result<Self, FromSDKError> {
        let config = aws_config::defaults(BehaviorVersion::latest()).load().await;
        let s3_config = aws_sdk_s3::config::Builder::from(&config).build();

        let region = s3_config
            .region()
            .map(Region::to_string)
            .ok_or(FromSDKError::MissingRegion)?;
        let endpoint_url_str = config.endpoint_url().unwrap_or("https://s3.amazonaws.com");
        let endpoint_url = Url::parse(endpoint_url_str).map_err(FromSDKError::InvalidEndpoint)?;

        let Some(credentials_provider) = config.credentials_provider() else {
            return Err(FromSDKError::NoCredentialsProvider);
        };

        Ok(Self {
            endpoint_url,
            region,
            // Address style is not exposed in the AWS SDK config, so we use the default.
            // See: <https://github.com/awslabs/aws-sdk-rust/issues/1230>
            addressing_style: S3AddressingStyle::default(),
            credentials_provider,
        })
    }

    /// The provider of the credentials, to hand to the AWS SDK through
    /// [`aws_sdk_s3::config::Builder::credentials_provider`].
    pub fn credentials_provider(&self) -> &SharedCredentialsProvider {
        &self.credentials_provider
    }

    /// Ask the provider for a currently valid set of credentials.
    ///
    /// Note that the result is a snapshot: temporary credentials in it expire,
    /// and nothing refreshes them. Keep the source itself around for anything
    /// that runs longer than a few hours.
    pub async fn credentials(&self) -> Result<ResolvedS3Credentials, CredentialsError> {
        let credentials = self.credentials_provider.provide_credentials().await?;

        Ok(ResolvedS3Credentials {
            endpoint_url: self.endpoint_url.clone(),
            region: self.region.clone(),
            addressing_style: self.addressing_style,
            access_key_id: credentials.access_key_id().to_string(),
            secret_access_key: credentials.secret_access_key().to_string(),
            session_token: credentials.session_token().map(ToString::to_string),
        })
    }
}

impl From<ResolvedS3Credentials> for S3CredentialSource {
    fn from(credentials: ResolvedS3Credentials) -> Self {
        Self {
            endpoint_url: credentials.endpoint_url,
            region: credentials.region,
            addressing_style: credentials.addressing_style,
            // We only know the access keys, not when they stop working, so they
            // are handed out as-is for as long as the process runs.
            credentials_provider: SharedCredentialsProvider::new(Credentials::new(
                credentials.access_key_id,
                credentials.secret_access_key,
                credentials.session_token,
                None,
                "rattler",
            )),
        }
    }
}

impl S3Credentials {
    /// Try to resolve the S3 credentials using the provided authentication
    /// storage.
    pub fn resolve(
        self,
        bucket_url: &Url,
        auth_storage: &AuthenticationStorage,
    ) -> Option<ResolvedS3Credentials> {
        let (access_key_id, secret_access_key, session_token) =
            if let (Some(access_key_id), Some(secret_access_key)) =
                (self.access_key_id, self.secret_access_key)
            {
                (access_key_id, secret_access_key, self.session_token)
            } else if let Some((access_key_id, secret_access_key, session_token)) =
                load_s3_credentials_from_auth_storage(auth_storage, bucket_url.clone())
            {
                // Use the credentials from the authentication storage if they are available.
                (access_key_id, secret_access_key, session_token)
            } else {
                return None;
            };

        Some(ResolvedS3Credentials {
            endpoint_url: self.endpoint_url,
            region: self.region,
            access_key_id,
            secret_access_key,
            session_token,
            addressing_style: self.addressing_style,
        })
    }
}

fn load_s3_credentials_from_auth_storage(
    auth_storage: &AuthenticationStorage,
    channel: Url,
) -> Option<(String, String, Option<String>)> {
    let auth = auth_storage.get_by_url(channel).ok()?;
    if let (
        _,
        Some(Authentication::S3Credentials {
            access_key_id,
            secret_access_key,
            session_token,
        }),
    ) = auth
    {
        Some((access_key_id, secret_access_key, session_token))
    } else {
        None
    }
}
