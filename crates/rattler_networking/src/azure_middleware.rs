//! Middleware to handle `az://` URLs to pull artifacts from Azure Blob Storage.
use std::collections::HashMap;

use async_trait::async_trait;
use rattler_config::config::azure::AzureOptions;
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

/// Middleware that rewrites `az://{container}/{path}` URLs to HTTPS Azure Blob
/// Storage URLs and signs them via reqsign's Azure `DefaultCredentialProvider`.
#[derive(Clone)]
pub struct AzureMiddleware {
    /// Container name -> addressing options (account, optional endpoint).
    config: HashMap<String, AzureOptions>,
    /// reqsign signer; caches the resolved credential internally.
    signer: Signer<Credential>,
}

impl AzureMiddleware {
    /// Create a new Azure middleware from a container -> options map.
    pub fn new(config: HashMap<String, AzureOptions>) -> Self {
        let client = Client::new();
        let ctx = Context::new()
            .with_file_read(TokioFileRead)
            .with_http_send(ReqwestHttpSend::new(client))
            .with_command_execute(TokioCommandExecute)
            .with_env(OsEnv);
        let signer = Signer::new(ctx, DefaultCredentialProvider::new(), RequestSigner::new());
        Self { config, signer }
    }

    /// Resolve the HTTPS base host for a container from config.
    /// Returns the endpoint origin, e.g. `https://acct.blob.core.windows.net`.
    fn endpoint_for(&self, container: &str) -> MiddlewareResult<Url> {
        let options = self.config.get(container).ok_or_else(|| {
            reqwest_middleware::Error::Middleware(anyhow::anyhow!(
                "no azure-options configured for container '{container}'"
            ))
        })?;
        let endpoint = match &options.endpoint_url {
            Some(url) => url.clone(),
            None => Url::parse(&format!(
                "https://{}.blob.core.windows.net",
                options.account
            ))
            .map_err(|e| reqwest_middleware::Error::Middleware(anyhow::anyhow!(e)))?,
        };
        Ok(endpoint)
    }

    /// Rewrite an `az://{container}/{path}` URL to its HTTPS equivalent.
    fn rewrite_url(&self, az_url: &Url) -> MiddlewareResult<Url> {
        let container = az_url.host_str().ok_or_else(|| {
            reqwest_middleware::Error::Middleware(anyhow::anyhow!(
                "container should be present in az URL, got: {az_url}"
            ))
        })?;
        let endpoint = self.endpoint_for(container)?;
        let new_url = format!(
            "{}/{}{}",
            endpoint.as_str().trim_end_matches('/'),
            container,
            az_url.path()
        );
        Url::parse(&new_url).map_err(|e| {
            reqwest_middleware::Error::Middleware(anyhow::anyhow!(
                "failed to parse constructed azure URL '{new_url}': {e}"
            ))
        })
    }
}

impl AzureMiddleware {
    /// Sign a reqwest `Request` in place using reqsign.
    async fn sign(&self, req: &mut Request) -> MiddlewareResult<()> {
        if !req.headers().contains_key("x-ms-version") {
            req.headers_mut()
                .insert("x-ms-version", X_MS_VERSION.parse().unwrap());
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
        if req.url().scheme() == "az" {
            let https_url = self.rewrite_url(&req.url().clone())?;
            *req.url_mut() = https_url;
            self.sign(&mut req).await?;
        }
        next.run(req, extensions).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn opts(account: &str, endpoint: Option<&str>) -> AzureOptions {
        AzureOptions {
            account: account.to_string(),
            endpoint_url: endpoint.map(|e| Url::parse(e).unwrap()),
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
