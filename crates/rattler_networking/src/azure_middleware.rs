//! Middleware to handle `az://` URLs to pull artifacts from Azure Blob Storage.
use async_trait::async_trait;
use reqsign_azure_storage::{Credential, DefaultCredentialProvider, RequestSigner};
use reqsign_command_execute_tokio::TokioCommandExecute;
use reqsign_core::{Context, ErrorKind, OsEnv, ProvideCredential, Signer};
use reqsign_file_read_tokio::TokioFileRead;
use reqsign_http_send_reqwest::ReqwestHttpSend;
use reqwest::{Client, Request, Response};
use reqwest_middleware::{Middleware, Next, Result as MiddlewareResult};
use url::Url;

/// The Azure Storage REST API version sent on every request.
const X_MS_VERSION: &str = "2021-12-02";

/// Middleware that rewrites `az://` URLs to HTTPS Azure Blob Storage URLs and
/// signs them.
///
/// The `az://` URL carries the full blob endpoint in its host, so rewriting is a
/// plain scheme swap: `az://{host}/{path}` → `https://{host}/{path}`. A conda
/// channel is therefore addressed the same way it is on the wire, e.g.
/// `az://myaccount.blob.core.windows.net/mycontainer` — no separate account or
/// endpoint configuration is needed. Sovereign clouds and emulators (Azurite)
/// work automatically because the endpoint is spelled out in the host.
///
/// Credentials are resolved by reqsign's [`DefaultCredentialProvider`] chain, in
/// its usual order: environment variables, then workload/managed identity, then
/// the Azure CLI (`az login`). rattler's [`crate::AuthenticationStorage`] is not
/// consulted for Azure — there is no `Authentication` Azure variant — so
/// per-host credentials configured there do not apply to `az://` requests.
///
/// When no credential resolves at all, the request is sent **unsigned** rather
/// than failing, so public/anonymous containers remain reachable with zero
/// ambient credentials.
#[derive(Clone)]
pub struct AzureMiddleware {
    /// reqsign signer; caches the resolved credential internally.
    signer: Signer<Credential>,
}

impl AzureMiddleware {
    /// Create a new Azure middleware.
    ///
    /// `client` is used for reqsign's credential resolution (IMDS / managed
    /// identity / AAD token fetches), so it must be the caller's configured
    /// client — proxy, CA bundle, and TLS settings carry through to those
    /// requests.
    pub fn new(client: Client) -> Self {
        Self::with_credential_provider(client, DefaultCredentialProvider::new())
    }

    /// Build the middleware around an explicit credential provider.
    ///
    /// [`AzureMiddleware::new`] wires up the [`DefaultCredentialProvider`] chain;
    /// tests use this seam to inject a deterministic provider (e.g. an empty
    /// chain, or a static key) without touching the ambient environment.
    fn with_credential_provider(
        client: Client,
        provider: impl ProvideCredential<Credential = Credential> + 'static,
    ) -> Self {
        let ctx = Context::new()
            .with_file_read(TokioFileRead)
            .with_http_send(ReqwestHttpSend::new(client))
            .with_command_execute(TokioCommandExecute)
            .with_env(OsEnv);
        let signer = Signer::new(ctx, provider, RequestSigner::new());
        Self { signer }
    }

    /// Rewrite an `az://{host}/{path}` URL to its HTTPS equivalent by swapping
    /// the scheme. Host, path, query and fragment are preserved verbatim.
    fn rewrite_url(az_url: &Url) -> MiddlewareResult<Url> {
        let https = az_url.as_str().replacen("az://", "https://", 1);
        Url::parse(&https).map_err(|e| {
            reqwest_middleware::Error::Middleware(anyhow::anyhow!(
                "failed to parse constructed azure URL '{https}': {e}"
            ))
        })
    }

    /// Whether the URL already carries an explicit SAS token (a `sig` query
    /// parameter). Such a URL is self-authenticating and must not be re-signed.
    fn has_sas_token(url: &Url) -> bool {
        url.query_pairs().any(|(key, _)| key == "sig")
    }

    /// Sign a reqwest `Request` in place using reqsign.
    ///
    /// Two cases short-circuit without touching the request:
    /// - The URL already carries an explicit SAS (`?...&sig=...`). Signing would
    ///   add an `Authorization` header that Azure prefers over the SAS, silently
    ///   overriding the caller's explicit token.
    /// - No credential resolves. reqsign surfaces this as
    ///   [`ErrorKind::CredentialInvalid`]; the request is then sent unsigned so
    ///   public/anonymous containers stay reachable.
    async fn sign(&self, req: &mut Request) -> MiddlewareResult<()> {
        if Self::has_sas_token(req.url()) {
            return Ok(());
        }

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

        match self.signer.sign(&mut parts, None).await {
            Ok(()) => {}
            Err(e) if e.kind() == ErrorKind::CredentialInvalid => {
                tracing::debug!("no Azure credential resolved; sending `az://` request unsigned");
                return Ok(());
            }
            Err(e) => return Err(reqwest_middleware::Error::Middleware(anyhow::anyhow!(e))),
        }

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

        let https_url = Self::rewrite_url(&req.url().clone())?;
        *req.url_mut() = https_url;
        self.sign(&mut req).await?;
        next.run(req, extensions).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn swaps_scheme_to_https() {
        let rewritten = AzureMiddleware::rewrite_url(
            &Url::parse("az://myacct.blob.core.windows.net/mychannel/noarch/repodata.json")
                .unwrap(),
        )
        .unwrap();
        assert_eq!(
            rewritten.as_str(),
            "https://myacct.blob.core.windows.net/mychannel/noarch/repodata.json"
        );
    }

    #[test]
    fn preserves_query_and_fragment() {
        let rewritten = AzureMiddleware::rewrite_url(
            &Url::parse("az://acct.blob.core.windows.net/c/x.json?sv=2021&sig=abc#frag").unwrap(),
        )
        .unwrap();
        assert_eq!(
            rewritten.as_str(),
            "https://acct.blob.core.windows.net/c/x.json?sv=2021&sig=abc#frag"
        );
    }

    #[test]
    fn rewrites_azurite_style_host_and_port() {
        let rewritten = AzureMiddleware::rewrite_url(
            &Url::parse("az://127.0.0.1:10000/devstoreaccount1/noarch/repodata.json").unwrap(),
        )
        .unwrap();
        assert_eq!(
            rewritten.as_str(),
            "https://127.0.0.1:10000/devstoreaccount1/noarch/repodata.json"
        );
    }

    #[tokio::test]
    async fn passes_through_non_az_schemes_unchanged() {
        use reqwest_middleware::ClientBuilder;
        let client = ClientBuilder::new(Client::new())
            .with(AzureMiddleware::new(Client::new()))
            .build();
        // A non-`az` request must not be rewritten; it should be attempted as-is
        // (and fail on DNS), proving the middleware left it untouched.
        let result = client
            .get("https://this-host-does-not-exist.invalid/x")
            .send()
            .await;
        assert!(result.is_err());
    }

    #[test]
    fn detects_sas_token_in_query() {
        assert!(AzureMiddleware::has_sas_token(
            &Url::parse("https://acct.blob.core.windows.net/c/x.json?sv=2021&sig=abc").unwrap()
        ));
        assert!(!AzureMiddleware::has_sas_token(
            &Url::parse("https://acct.blob.core.windows.net/c/x.json?sv=2021").unwrap()
        ));
    }

    /// With no credential resolvable, a signable `az://` request must be passed
    /// through UNSIGNED (not errored), so public/anonymous containers work with
    /// zero ambient credentials. An empty provider chain resolves nothing, which
    /// reqsign reports as `CredentialInvalid`.
    #[tokio::test]
    async fn passes_request_through_unsigned_when_no_credential() {
        use reqsign_core::ProvideCredentialChain;

        let middleware = AzureMiddleware::with_credential_provider(
            Client::new(),
            ProvideCredentialChain::<Credential>::new(),
        );
        let mut req = Client::new()
            .get("https://acct.blob.core.windows.net/pub/noarch/repodata.json")
            .build()
            .unwrap();

        middleware
            .sign(&mut req)
            .await
            .expect("a request with no resolvable credential must pass through unsigned");

        assert!(
            req.headers().get(http::header::AUTHORIZATION).is_none(),
            "unsigned request must not carry an Authorization header"
        );
        assert!(
            !req.url().query_pairs().any(|(k, _)| k == "sig"),
            "unsigned request must not gain a SAS query parameter"
        );
    }

    /// A URL that already carries a SAS token must not be re-signed even when a
    /// credential is available: no `Authorization` header is added.
    #[tokio::test]
    async fn does_not_sign_url_that_already_has_sas() {
        use reqsign_azure_storage::StaticCredentialProvider;

        // A valid base64 account key so the static provider yields a usable
        // SharedKey credential that would otherwise sign the request.
        let middleware = AzureMiddleware::with_credential_provider(
            Client::new(),
            StaticCredentialProvider::new_shared_key("acct", "dGVzdF9rZXk="),
        );
        let mut req = Client::new()
            .get("https://acct.blob.core.windows.net/c/x.json?sv=2021&sig=abc")
            .build()
            .unwrap();

        middleware.sign(&mut req).await.unwrap();

        assert!(
            req.headers().get(http::header::AUTHORIZATION).is_none(),
            "a URL carrying an explicit SAS must not be re-signed"
        );
        assert!(
            !req.headers().contains_key("x-ms-version"),
            "a self-authenticating SAS URL is left untouched"
        );
    }
}
