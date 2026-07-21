//! Middleware to handle `az://` URLs to pull artifacts from Azure Blob Storage.
use std::collections::HashMap;

use async_trait::async_trait;
use reqsign_azure_storage::{Credential, DefaultCredentialProvider, RequestSigner};
use reqsign_command_execute_tokio::TokioCommandExecute;
use reqsign_core::{Context, OsEnv, Signer};
use reqsign_file_read_tokio::TokioFileRead;
use reqsign_http_send_reqwest::ReqwestHttpSend;
use reqwest::{Client, Request, Response};
use reqwest_middleware::{Middleware, Next, Result as MiddlewareResult};
use url::Url;

/// The Azure Storage REST API version sent on every request.
const X_MS_VERSION: &str = "2021-12-02";

/// Per-container addressing configuration for the Azure middleware.
///
/// Like [`crate::s3_middleware::S3Config`] this is an enum, so exactly one of
/// the two addressing modes is representable: either a storage account (from
/// which the default endpoint is derived) or a full endpoint override. It holds
/// only the information needed to *address* a container; credentials are
/// resolved separately at request time via reqsign's
/// [`DefaultCredentialProvider`], so they are kept out of this type.
///
/// Unlike `S3Config`, there is no default-provider fallback: a container that is
/// not present in the middleware's config map is an error at request time.
#[derive(Clone, Debug)]
pub enum AzureConfig {
    /// Address the container via `https://{account}.blob.core.windows.net`.
    Account(String),
    /// Address the container via a full endpoint override (sovereign clouds /
    /// custom endpoints such as Azurite).
    Endpoint(Url),
}

#[cfg(feature = "rattler_config")]
/// Compute the Azure configuration from the given Azure options.
pub fn compute_azure_config<M>(azure_options: &M) -> HashMap<String, AzureConfig>
where
    M: IntoIterator<Item = (String, rattler_config::config::azure::AzureOptions)> + Clone,
{
    azure_options
        .clone()
        .into_iter()
        .map(|(k, v)| {
            (
                k,
                v.endpoint_url
                    .map_or_else(|| AzureConfig::Account(v.account), AzureConfig::Endpoint),
            )
        })
        .collect()
}

#[cfg(feature = "rattler_config")]
/// Compute the Azure configuration from the `azure-options` of the shared
/// rattler configuration (see [`rattler_config`]).
///
/// Accepts a [`rattler_config::config::CommonConfig`]; a `&ConfigBase<T>` of
/// any extension coerces into it.
pub fn compute_azure_config_from_config(
    config: &rattler_config::config::CommonConfig,
) -> HashMap<String, AzureConfig> {
    config
        .azure_options
        .0
        .iter()
        .map(|(container, options)| {
            (
                container.clone(),
                options.endpoint_url.clone().map_or_else(
                    || AzureConfig::Account(options.account.clone()),
                    AzureConfig::Endpoint,
                ),
            )
        })
        .collect()
}

/// Middleware that rewrites `az://{container}/{path}` URLs to HTTPS Azure Blob
/// Storage URLs and signs them.
///
/// Credentials are resolved by reqsign's [`DefaultCredentialProvider`] chain, in
/// its usual order: environment variables, then workload/managed identity, then
/// the Azure CLI (`az login`). rattler's [`crate::AuthenticationStorage`] is not
/// consulted for Azure — there is no `Authentication` Azure variant — so
/// per-host credentials configured there do not apply to `az://` requests.
#[derive(Clone)]
pub struct AzureMiddleware {
    /// Container name -> addressing config (account or full endpoint override).
    config: HashMap<String, AzureConfig>,
    /// reqsign signer; caches the resolved credential internally.
    signer: Signer<Credential>,
}

impl AzureMiddleware {
    /// Create a new Azure middleware from a container -> config map.
    pub fn new(config: HashMap<String, AzureConfig>) -> Self {
        tracing::trace!("Creating Azure middleware using {:?}", config);
        let client = Client::new();
        let ctx = Context::new()
            .with_file_read(TokioFileRead)
            .with_http_send(ReqwestHttpSend::new(client))
            .with_command_execute(TokioCommandExecute)
            .with_env(OsEnv);
        let signer = Signer::new(ctx, DefaultCredentialProvider::new(), RequestSigner::new());
        Self { config, signer }
    }

    /// Resolve the endpoint base URL for a container from config.
    ///
    /// For an [`AzureConfig::Account`] this is the endpoint origin
    /// `https://{account}.blob.core.windows.net`. For an
    /// [`AzureConfig::Endpoint`] the configured override is returned verbatim,
    /// which may carry a path component (e.g. Azurite endpoints).
    fn endpoint_for(&self, container: &str) -> MiddlewareResult<Url> {
        let config = self.config.get(container).ok_or_else(|| {
            reqwest_middleware::Error::Middleware(anyhow::anyhow!(
                "no azure-options configured for container '{container}'"
            ))
        })?;
        match config {
            AzureConfig::Endpoint(url) => Ok(url.clone()),
            AzureConfig::Account(account) => {
                Url::parse(&format!("https://{account}.blob.core.windows.net"))
                    .map_err(|e| reqwest_middleware::Error::Middleware(anyhow::anyhow!(e)))
            }
        }
    }

    /// Rewrite an `az://{container}/{path}` URL to its HTTPS equivalent.
    fn rewrite_url(&self, az_url: &Url) -> MiddlewareResult<Url> {
        let container = az_url.host_str().ok_or_else(|| {
            reqwest_middleware::Error::Middleware(anyhow::anyhow!(
                "container should be present in az URL, got: {az_url}"
            ))
        })?;
        let endpoint = self.endpoint_for(container)?;
        let query = az_url.query().map(|q| format!("?{q}")).unwrap_or_default();
        let fragment = az_url
            .fragment()
            .map(|f| format!("#{f}"))
            .unwrap_or_default();
        let new_url = format!(
            "{}/{}{}{}{}",
            endpoint.as_str().trim_end_matches('/'),
            container,
            az_url.path(),
            query,
            fragment
        );
        Url::parse(&new_url).map_err(|e| {
            reqwest_middleware::Error::Middleware(anyhow::anyhow!(
                "failed to parse constructed azure URL '{new_url}': {e}"
            ))
        })
    }

    /// Sign a reqwest `Request` in place using reqsign.
    async fn sign(&self, req: &mut Request) -> MiddlewareResult<()> {
        if !req.headers().contains_key("x-ms-version") {
            req.headers_mut()
                .insert("x-ms-version", http::HeaderValue::from_static(X_MS_VERSION));
        }

        let mut builder = http::Request::builder()
            .method(req.method().clone())
            .uri(req.url().as_str());
        for (name, value) in req.headers() {
            builder = builder.header(name, value);
        }
        let http_req = builder.body(()).map_err(|e| {
            reqwest_middleware::Error::Middleware(anyhow::anyhow!(
                "failed to build http request for signing: {e}"
            ))
        })?;
        let (mut parts, ()) = http_req.into_parts();

        self.signer
            .sign(&mut parts, None)
            .await
            .map_err(|e| reqwest_middleware::Error::Middleware(anyhow::anyhow!(e)))?;

        *req.headers_mut() = parts.headers;
        let signed_url = Url::parse(&parts.uri.to_string()).map_err(|e| {
            reqwest_middleware::Error::Middleware(anyhow::anyhow!(
                "failed to parse signed azure URL '{}': {e}",
                parts.uri
            ))
        })?;
        *req.url_mut() = signed_url;
        Ok(())
    }
}

#[async_trait]
impl Middleware for AzureMiddleware {
    async fn handle(
        &self,
        mut req: Request,
        extensions: &mut http::Extensions,
        next: Next<'_>,
    ) -> MiddlewareResult<Response> {
        // Only intercept `az://` requests.
        if req.url().scheme() != "az" {
            return next.run(req, extensions).await;
        }

        let https_url = self.rewrite_url(&req.url().clone())?;
        *req.url_mut() = https_url;
        self.sign(&mut req).await?;
        next.run(req, extensions).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn opts(account: &str, endpoint: Option<&str>) -> AzureConfig {
        match endpoint {
            Some(e) => AzureConfig::Endpoint(Url::parse(e).unwrap()),
            None => AzureConfig::Account(account.to_string()),
        }
    }

    #[test]
    fn rewrites_default_endpoint() {
        let mw = AzureMiddleware::new(HashMap::from([(
            "mychannel".to_string(),
            opts("myacct", None),
        )]));
        let rewritten = mw
            .rewrite_url(&Url::parse("az://mychannel/noarch/repodata.json").unwrap())
            .unwrap();
        assert_eq!(
            rewritten.as_str(),
            "https://myacct.blob.core.windows.net/mychannel/noarch/repodata.json"
        );
    }

    #[test]
    fn rewrites_override_endpoint_for_azurite() {
        let mw = AzureMiddleware::new(HashMap::from([(
            "devstoreaccount1".to_string(),
            opts(
                "devstoreaccount1",
                Some("http://127.0.0.1:10000/devstoreaccount1"),
            ),
        )]));
        let rewritten = mw
            .rewrite_url(&Url::parse("az://devstoreaccount1/noarch/repodata.json").unwrap())
            .unwrap();
        assert_eq!(
            rewritten.as_str(),
            "http://127.0.0.1:10000/devstoreaccount1/devstoreaccount1/noarch/repodata.json"
        );
    }

    #[test]
    fn errors_when_container_not_configured() {
        let mw = AzureMiddleware::new(HashMap::new());
        let err = mw
            .rewrite_url(&Url::parse("az://missing/noarch/repodata.json").unwrap())
            .unwrap_err();
        assert!(err.to_string().contains("missing"));
    }

    #[tokio::test]
    async fn passes_through_non_az_schemes_unchanged() {
        use reqwest_middleware::ClientBuilder;
        let mw = AzureMiddleware::new(HashMap::new());
        let client = ClientBuilder::new(Client::new()).with(mw).build();
        let result = client
            .get("https://this-host-does-not-exist.invalid/x")
            .send()
            .await;
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(
            !err.to_string().contains("azure-options"),
            "non-az request must not hit azure config lookup: {err}"
        );
    }
}
