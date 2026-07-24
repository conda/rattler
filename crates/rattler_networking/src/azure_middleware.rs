//! Middleware to handle `az://` URLs to pull artifacts from Azure Blob Storage.
use async_trait::async_trait;
use reqsign_azure_storage::{Credential, DefaultCredentialProvider, RequestSigner};
use reqsign_command_execute_tokio::TokioCommandExecute;
use reqsign_core::{Context, OsEnv, ProvideCredential, Signer};
use reqsign_file_read_tokio::TokioFileRead;
use reqsign_http_send_reqwest::ReqwestHttpSend;
use reqwest::{Client, Request, Response};
use reqwest_middleware::{Middleware, Next, Result as MiddlewareResult};
use url::Url;

/// The Azure Storage REST API version sent on every request.
const X_MS_VERSION: &str = "2021-12-02";

/// Whether an Azure credential source appears configured in the process
/// environment.
///
/// Used to distinguish "no credential at all" (fall back to an unsigned,
/// anonymous request) from "a credential is configured but signing failed"
/// (a hard error). It is a presence check only — it does not validate that the
/// values are usable; reqsign does that when it actually signs.
///
/// A persisted `az login` session (a profile on disk) counts as a source too;
/// see [`azure_cli_session_present`].
///
/// This result gates whether credentials are resolved at all: when it is
/// `false`, [`AzureMiddleware::sign`] sends the request unsigned *without*
/// probing, so anonymous public-container reads do not block on the
/// managed-identity / IMDS timeout. The tradeoff is that a *bare* system-assigned
/// managed identity — one reached only via the IMDS endpoint with no
/// `IDENTITY_ENDPOINT` / `MSI_ENDPOINT` / federated-token env var set — is not
/// detected here, so its requests go unsigned. App Service, Functions, Cloud
/// Shell and AKS workload identity all set one of those env vars and are
/// unaffected; a bare `IaaS` VM identity must export `MSI_ENDPOINT` (or supply an
/// explicit credential) to be used.
fn azure_credential_source_present() -> bool {
    // Explicit Shared Key or SAS token.
    if std::env::var_os("AZURE_STORAGE_ACCOUNT_KEY").is_some()
        || std::env::var_os("AZURE_STORAGE_SAS_TOKEN").is_some()
    {
        return true;
    }
    // Service-principal flows need both a client and a tenant to be meaningful.
    if std::env::var_os("AZURE_CLIENT_ID").is_some()
        && std::env::var_os("AZURE_TENANT_ID").is_some()
    {
        return true;
    }
    // Workload identity federation (e.g. AKS).
    if std::env::var_os("AZURE_FEDERATED_TOKEN_FILE").is_some() {
        return true;
    }
    // Managed identity endpoints (App Service / Functions / Cloud Shell / IMDS
    // override).
    if std::env::var_os("MSI_ENDPOINT").is_some() || std::env::var_os("IDENTITY_ENDPOINT").is_some()
    {
        return true;
    }
    // Persisted `az login` session.
    azure_cli_session_present()
}

/// Whether a persisted `az login` session exists on disk.
///
/// `az login` writes an `azureProfile.json` into the Azure CLI config dir
/// (`$AZURE_CONFIG_DIR`, else `~/.azure`). Its presence means a login was
/// performed at some point, so a *broken* az-login credential hard-errors
/// rather than silently downgrading to an unsigned request. This is a presence
/// check only — reqsign validates the token when it actually signs, so a stale
/// profile with no usable token still yields a hard error, never anonymous.
fn azure_cli_session_present() -> bool {
    let config_dir = match std::env::var_os("AZURE_CONFIG_DIR") {
        Some(dir) => std::path::PathBuf::from(dir),
        None => match std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE")) {
            Some(home) => std::path::PathBuf::from(home).join(".azure"),
            None => return false,
        },
    };
    config_dir.join("azureProfile.json").exists()
}

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
/// # Trust model
///
/// The URL host is **trusted verbatim**: it becomes the HTTPS request target
/// unchanged, with no allow-list of accounts or endpoints. Whatever ambient
/// AAD / Shared-Key credential resolves below is applied to that host — so a
/// channel author who controls the `az://` URL controls where the request, and
/// any credential material, is sent. Because of this, **userinfo is rejected**:
/// an `az://user:pass@host/...` authority is a host-spoofing vector (the real
/// host can be hidden behind userinfo) and such requests are refused before any
/// rewrite or signing.
///
/// Credentials are resolved by reqsign's [`DefaultCredentialProvider`] chain, in
/// its usual order: environment variables, then workload/managed identity, then
/// the Azure CLI (`az login`). rattler's [`crate::AuthenticationStorage`] is not
/// consulted for Azure — there is no `Authentication` Azure variant — so
/// per-host credentials configured there do not apply to `az://` requests.
///
/// When **no credential source is detected**, the request is sent **unsigned**
/// rather than failing, so public/anonymous containers remain reachable with
/// zero ambient credentials. When a credential source *is* configured
/// (see `azure_credential_source_present`) but signing fails, that is a
/// **hard error** — reqsign collapses "no credential" and "broken credential"
/// into the same [`reqsign_core::ErrorKind::CredentialInvalid`], so a broken credential must
/// not be silently downgraded to an anonymous request.
#[derive(Clone)]
pub struct AzureMiddleware {
    /// reqsign signer; caches the resolved credential internally.
    signer: Signer<Credential>,
    /// Whether an Azure credential source appears configured in the process
    /// environment. Captured at construction. When `true`, a signing failure is
    /// propagated as a hard error instead of falling back to an unsigned
    /// request.
    credential_source_present: bool,
}

impl AzureMiddleware {
    /// Create a new Azure middleware.
    ///
    /// `client` is used for reqsign's credential resolution (IMDS / managed
    /// identity / AAD token fetches), so it must be the caller's configured
    /// client — proxy, CA bundle, and TLS settings carry through to those
    /// requests.
    pub fn new(client: Client) -> Self {
        Self::with_credential_provider(
            client,
            DefaultCredentialProvider::new(),
            azure_credential_source_present(),
        )
    }

    /// Build the middleware around an explicit credential provider.
    ///
    /// [`AzureMiddleware::new`] wires up the [`DefaultCredentialProvider`] chain
    /// and detects the credential source from the environment; tests use this
    /// seam to inject a deterministic provider (e.g. an empty chain, or a static
    /// key) and an explicit `credential_source_present` flag without touching the
    /// ambient environment.
    fn with_credential_provider(
        client: Client,
        provider: impl ProvideCredential<Credential = Credential> + 'static,
        credential_source_present: bool,
    ) -> Self {
        let ctx = Context::new()
            .with_file_read(TokioFileRead)
            .with_http_send(ReqwestHttpSend::new(client))
            .with_command_execute(TokioCommandExecute)
            .with_env(OsEnv);
        let signer = Signer::new(ctx, provider, RequestSigner::new());
        Self {
            signer,
            credential_source_present,
        }
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

    /// Whether the URL carries userinfo (`user` and/or `:pass` before the host).
    /// Because the host is trusted verbatim, a `user:pass@host` authority is a
    /// host-spoofing vector and must be refused. Userinfo in a blob URL is invalid
    /// regardless and safe to ignore.
    fn has_userinfo(url: &Url) -> bool {
        !url.username().is_empty() || url.password().is_some()
    }

    /// Sign a reqwest `Request` in place using reqsign.
    ///
    /// Two cases short-circuit without invoking reqsign at all:
    /// - The URL already carries an explicit SAS (`?...&sig=...`). Signing would
    ///   add an `Authorization` header that Azure prefers over the SAS, silently
    ///   overriding the caller's explicit token.
    /// - No credential source was detected (`credential_source_present` is
    ///   `false`). The request is sent unsigned so public/anonymous containers
    ///   stay reachable — and crucially, credential *resolution* is skipped
    ///   entirely. Otherwise reqsign would probe the managed-identity / IMDS
    ///   endpoint and block until it times out (~30s on a machine with no
    ///   metadata service) before we could fall back, making every anonymous
    ///   public-channel read pay that timeout.
    ///
    /// If a credential source *is* configured but signing fails with
    /// [`reqsign_core::ErrorKind::CredentialInvalid`] (a broken key/token, not
    /// an absent one), the error is propagated rather than downgraded to an
    /// unsigned request.
    async fn sign(&self, req: &mut Request) -> MiddlewareResult<()> {
        if Self::has_sas_token(req.url()) {
            return Ok(());
        }

        if !req.headers().contains_key("x-ms-version") {
            req.headers_mut()
                .insert("x-ms-version", http::HeaderValue::from_static(X_MS_VERSION));
        }

        // No credential source detected: send unsigned without probing. See the
        // doc comment above — probing here would block on the IMDS timeout.
        if !self.credential_source_present {
            tracing::debug!(
                "no Azure credential source detected; sending `az://` request unsigned"
            );
            return Ok(());
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

        // A credential source is present (the absent case short-circuited to
        // unsigned above). reqsign reports a broken key/token as
        // `CredentialInvalid`; that must surface as a hard error rather than
        // silently going anonymous, so any signing failure is propagated.
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

        // The host is trusted verbatim as the request target, so userinfo is a
        // host-spoofing vector (`az://user:pass@real.host/...` can hide the real
        // authority). Reject it before rewriting or signing. This mirrors the
        // same rejection in `rattler_azure::account_and_container`; the check is
        // inlined here because this middleware does not depend on rattler_azure.
        if Self::has_userinfo(req.url()) {
            return Err(reqwest_middleware::Error::Middleware(anyhow::anyhow!(
                "userinfo is not allowed in `az://` URLs (host-spoofing vector); \
                 remove the `user:pass@` component"
            )));
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
            false,
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
            true,
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

    /// A URL carrying userinfo must be recognised so the fetch path can reject
    /// it: the host is trusted verbatim, so `user:pass@host` is a host-spoofing
    /// vector. (A request built through reqwest's client strips userinfo into a
    /// header before the middleware runs, so the predicate — not the whole
    /// client path — is what guards direct `Request` construction.)
    #[test]
    fn detects_userinfo_in_url() {
        assert!(AzureMiddleware::has_userinfo(
            &Url::parse("az://user:pass@acct.blob.core.windows.net/c/x.json").unwrap()
        ));
        assert!(AzureMiddleware::has_userinfo(
            &Url::parse("az://user@acct.blob.core.windows.net/c/x.json").unwrap()
        ));
        assert!(!AzureMiddleware::has_userinfo(
            &Url::parse("az://acct.blob.core.windows.net/c/x.json").unwrap()
        ));
    }

    /// With no credential source detected, `sign` must NOT invoke the credential
    /// provider at all — resolution is skipped so anonymous reads don't block on
    /// the IMDS timeout. Uses a provider that flips a flag if it is ever asked.
    #[tokio::test]
    async fn skips_credential_resolution_when_no_source() {
        use std::sync::Arc;
        use std::sync::atomic::{AtomicBool, Ordering};

        #[derive(Debug)]
        struct RecordingProvider(Arc<AtomicBool>);
        impl ProvideCredential for RecordingProvider {
            type Credential = Credential;
            async fn provide_credential(
                &self,
                _ctx: &Context,
            ) -> reqsign_core::Result<Option<Credential>> {
                self.0.store(true, Ordering::SeqCst);
                Ok(None)
            }
        }

        let probed = Arc::new(AtomicBool::new(false));
        let middleware = AzureMiddleware::with_credential_provider(
            Client::new(),
            RecordingProvider(probed.clone()),
            false,
        );
        let mut req = Client::new()
            .get("https://acct.blob.core.windows.net/pub/noarch/repodata.json")
            .build()
            .unwrap();

        middleware.sign(&mut req).await.unwrap();

        assert!(
            !probed.load(Ordering::SeqCst),
            "credential provider must not be probed when no source is detected"
        );
        assert!(
            req.headers().get(http::header::AUTHORIZATION).is_none(),
            "unsigned request must not carry an Authorization header"
        );
    }

    /// A persisted `az login` profile counts as a credential source (so a
    /// broken az-login session hard-errors instead of going anonymous), while
    /// an empty config dir does not.
    #[test]
    fn detects_az_login_profile_on_disk() {
        let dir = tempfile::tempdir().unwrap();
        temp_env::with_var("AZURE_CONFIG_DIR", Some(dir.path().as_os_str()), || {
            assert!(
                !azure_cli_session_present(),
                "no profile file yet ⇒ not a credential source"
            );
            std::fs::write(dir.path().join("azureProfile.json"), "{}").unwrap();
            assert!(
                azure_cli_session_present(),
                "azureProfile.json present ⇒ credential source"
            );
        });
    }

    /// When a credential source is detected but signing fails
    /// (`CredentialInvalid`), the failure must be a hard error rather than an
    /// unsigned fallback: a broken credential must not silently go anonymous. An
    /// empty provider chain yields `CredentialInvalid`, standing in for a broken
    /// credential.
    #[tokio::test]
    async fn errors_when_credential_source_present_but_signing_fails() {
        use reqsign_core::ProvideCredentialChain;

        let middleware = AzureMiddleware::with_credential_provider(
            Client::new(),
            ProvideCredentialChain::<Credential>::new(),
            true,
        );
        let mut req = Client::new()
            .get("https://acct.blob.core.windows.net/c/noarch/repodata.json")
            .build()
            .unwrap();

        let result = middleware.sign(&mut req).await;

        assert!(
            result.is_err(),
            "a configured-but-failing credential must be a hard error, not unsigned"
        );
        assert!(
            req.headers().get(http::header::AUTHORIZATION).is_none(),
            "a failed signing attempt must not leave a partial Authorization header"
        );
    }
}
