//! Glue between the credentials of the AWS SDK and the request signer that
//! `opendal` uses for S3.

use std::time::{SystemTime, UNIX_EPOCH};

use aws_credential_types::provider::{ProvideCredentials, SharedCredentialsProvider};
use opendal::{
    Configurator,
    services::{S3, S3Config},
};
use reqsign_core::{ProvideCredentialChain, time::Timestamp};

use crate::{S3AddressingStyle, S3CredentialSource};

impl S3CredentialSource {
    /// Create an `opendal` S3 builder for `bucket`, rooted at `root`.
    ///
    /// Requests are signed with the credentials of this source. `opendal` asks
    /// for a new set whenever the ones it holds are about to expire, so an
    /// operator built this way keeps working past the lifetime of the temporary
    /// credentials of AWS SSO, an assumed role or the instance metadata service.
    pub fn opendal_builder(&self, bucket: &str, root: &str) -> S3 {
        let mut config = S3Config::default();
        config.bucket = bucket.to_string();
        config.root = Some(root.to_string());
        config.region = Some(self.region.clone());
        config.endpoint = Some(self.endpoint_url.to_string());
        config.enable_virtual_host_style = self.addressing_style == S3AddressingStyle::VirtualHost;
        // The settings are resolved already, so don't let `opendal` read the AWS
        // configuration a second time.
        config.disable_config_load = true;

        config.into_builder().credential_provider_chain(
            ProvideCredentialChain::new()
                .push(AwsSdkCredentialProvider(self.credentials_provider.clone())),
        )
    }
}

/// Hands the credentials of the AWS SDK to the request signer of `opendal`.
///
/// Every call reaches the AWS SDK, which renews whatever it needs to produce a
/// fresh set: it re-reads the credential file, re-assumes the role, re-queries
/// the instance metadata, or exchanges the refresh token of an `sso_session` for
/// a new SSO access token, depending on how it is configured.
#[derive(Debug)]
struct AwsSdkCredentialProvider(SharedCredentialsProvider);

impl reqsign_core::ProvideCredential for AwsSdkCredentialProvider {
    type Credential = reqsign_aws_v4::Credential;

    async fn provide_credential(
        &self,
        _ctx: &reqsign_core::Context,
    ) -> reqsign_core::Result<Option<Self::Credential>> {
        let credentials = self.0.provide_credentials().await.map_err(|error| {
            reqsign_core::Error::unexpected("failed to resolve the AWS credentials")
                .with_source(error)
        })?;

        Ok(Some(reqsign_aws_v4::Credential {
            access_key_id: credentials.access_key_id().to_string(),
            secret_access_key: credentials.secret_access_key().to_string(),
            session_token: credentials.session_token().map(ToString::to_string),
            // Credentials without an expiry are long-lived, and `opendal` keeps
            // using those for as long as the process runs.
            expires_in: credentials.expiry().map(to_timestamp).transpose()?,
        }))
    }
}

fn to_timestamp(time: SystemTime) -> reqsign_core::Result<Timestamp> {
    let since_epoch = time.duration_since(UNIX_EPOCH).map_err(|error| {
        reqsign_core::Error::unexpected("AWS credentials expire before the Unix epoch")
            .with_source(error)
    })?;

    Timestamp::from_millisecond(since_epoch.as_millis() as i64)
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use aws_credential_types::Credentials;
    use reqsign_core::{ProvideCredential, SigningCredential};

    use super::*;

    #[tokio::test]
    async fn temporary_credentials_carry_their_expiry_to_the_signer() {
        let expiry = SystemTime::now() + Duration::from_secs(12 * 60 * 60);
        let provider = SharedCredentialsProvider::new(Credentials::new(
            "ASIAEXAMPLE",
            "secret",
            Some("session-token".to_string()),
            Some(expiry),
            "test",
        ));

        let credential = AwsSdkCredentialProvider(provider)
            .provide_credential(&reqsign_core::Context::new())
            .await
            .expect("the provider should hand out credentials")
            .expect("the provider should hand out credentials");

        assert_eq!(credential.access_key_id, "ASIAEXAMPLE");
        assert_eq!(credential.secret_access_key, "secret");
        assert_eq!(credential.session_token.as_deref(), Some("session-token"));
        // The signer asks for new credentials once the expiry is near, so it has
        // to see it.
        assert!(credential.is_valid());
        assert!(!credential.is_valid_at(to_timestamp(expiry).unwrap()));
    }

    #[tokio::test]
    async fn long_lived_credentials_never_expire() {
        let provider = SharedCredentialsProvider::new(Credentials::new(
            "AKIAEXAMPLE",
            "secret",
            None,
            None,
            "test",
        ));

        let credential = AwsSdkCredentialProvider(provider)
            .provide_credential(&reqsign_core::Context::new())
            .await
            .expect("the provider should hand out credentials")
            .expect("the provider should hand out credentials");

        assert_eq!(credential.session_token, None);
        assert_eq!(credential.expires_in, None);
        assert!(credential.is_valid());
    }
}
