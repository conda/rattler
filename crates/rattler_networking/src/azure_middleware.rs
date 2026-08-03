//! Middleware to handle `az://` URLs to pull artifacts from Azure Blob Storage.
use std::collections::HashMap;

use async_trait::async_trait;
use rattler_azure::{Auth, AzureChannelUrl, AzureEndpointOptions, AzureHost};
use reqsign_azure_storage::{Credential, DefaultCredentialProvider, RequestSigner};
use reqsign_command_execute_tokio::TokioCommandExecute;
use reqsign_core::{Context, OsEnv, ProvideCredential, Signer};
use reqsign_file_read_tokio::TokioFileRead;
use reqsign_http_send_reqwest::ReqwestHttpSend;
use reqwest::{Client, Request, Response};
use reqwest_middleware::{Middleware, Next, Result as MiddlewareResult};
use url::Url;

/// The Azure Storage REST API version sent on every signed request. A URL that
/// already carries a SAS returns before this is attached, and the write path pins
/// its own version inside opendal.
const X_MS_VERSION: &str = "2021-12-02";

/// Middleware that rewrites `az://` URLs to their wire form and, where a host is
/// granted credentials, signs them.
///
/// The `az://` URL carries the full blob endpoint in its host, so rewriting is a
/// plain scheme swap: `az://{host}/{path}` → `https://{host}/{path}`. A conda
/// channel is therefore addressed the same way it is on the wire, e.g.
/// `az://myaccount.blob.core.windows.net/mycontainer` — no separate account or
/// endpoint configuration is needed. Sovereign clouds work with no configuration
/// at all; an emulator needs only the `scheme = "http"` line below.
///
/// # Trust model
///
/// **Anonymous by default.** With no entry for a host, its requests are sent
/// unsigned and *no credential is resolved at all* — so no ambient Azure
/// credential can leak to a host the user never named, and an anonymous read of a
/// public container does not block on the managed-identity / IMDS probe.
///
/// A credential attaches to a host only because an [`AzureEndpointOptions`] entry
/// for it says [`Auth::DefaultChain`], which comes from the user's `azure-options`
/// config table:
///
/// ```toml
/// [azure-options."mycompany.blob.core.windows.net"]
/// auth = true
///
/// [azure-options."127.0.0.1:10000"]   # Azurite
/// auth = true
/// scheme = "http"
/// ```
///
/// Two consequences of the grant being explicit:
///
/// - **Nothing is inferred from the host name.** There is no allow-list of
///   "official" Azure suffixes, and none is needed: a host nobody granted gets
///   nothing regardless of what it is called, and an entry for a custom host *is*
///   the declaration that the user trusts that endpoint.
/// - **A broken credential is a hard error.** Because the user asked for signing,
///   an unusable credential must be reported, not silently downgraded to an
///   anonymous request that Azure will answer with a confusing 404.
///
/// Entries are user-scoped by contract: a project- or workspace-level manifest
/// must never be allowed to write one, since that would let a checked-out
/// repository name a host and receive the user's credentials.
///
/// `az://user:pass@host/...` is refused outright. The host becomes the request
/// target verbatim, so userinfo is a host-spoofing vector — the real authority can
/// hide behind it — and userinfo is invalid in a blob URL anyway.
///
/// Granted credentials are resolved by reqsign's [`DefaultCredentialProvider`]
/// chain, in its order: environment variables, the Azure CLI (`az login`),
/// client certificate, client secret, pipelines, workload identity, IMDS.
/// rattler's [`crate::AuthenticationStorage`] is not consulted — it has no Azure
/// variant, and [`crate::AuthenticationMiddleware`] handles only `http`/`https`,
/// so its host-keyed entries cannot reach an `az://` request either.
#[derive(Clone)]
pub struct AzureMiddleware {
    /// reqsign signer; caches the resolved credential internally.
    signer: Signer<Credential>,

    /// Per-host endpoint options, keyed by the same normalized authority the
    /// `azure-options` config table is keyed by. An absent host is *defined* to
    /// behave as a defaulted entry (anonymous, https), so a miss is never a
    /// separate code path.
    ///
    /// A plain `HashMap` rather than `rattler_config::AzureOptionsMap`, mirroring
    /// [`crate::S3Middleware`]. No caller has a `rattler_config::Config` in hand
    /// today — every one of them passes an empty table — so taking the config type
    /// would buy a mandatory `rattler_config` edge on the `azure` feature for zero
    /// saved conversions. When a caller does grow one, add a
    /// `#[cfg(feature = "rattler_config")]` helper next to
    /// [`crate::s3_middleware::compute_s3_config_from_config`] rather than changing
    /// this signature.
    options: HashMap<AzureHost, AzureEndpointOptions>,
}

impl AzureMiddleware {
    /// Create a new Azure middleware.
    ///
    /// `client` is used for reqsign's credential resolution (IMDS / managed
    /// identity / AAD token fetches), so it must be the caller's configured
    /// client — proxy, CA bundle, and TLS settings carry through to those
    /// requests.
    ///
    /// `options` is the `azure-options` table: the per-host grants. An empty map
    /// means every `az://` request is anonymous.
    pub fn new(client: Client, options: HashMap<AzureHost, AzureEndpointOptions>) -> Self {
        Self::with_credential_provider(client, DefaultCredentialProvider::new(), options)
    }

    /// Build the middleware around an explicit credential provider.
    ///
    /// [`AzureMiddleware::new`] wires up the [`DefaultCredentialProvider`] chain;
    /// tests use this seam to inject a deterministic provider (an empty chain
    /// standing in for a broken credential, or a static key) without touching the
    /// ambient environment.
    fn with_credential_provider(
        client: Client,
        provider: impl ProvideCredential<Credential = Credential> + 'static,
        options: HashMap<AzureHost, AzureEndpointOptions>,
    ) -> Self {
        let ctx = Context::new()
            .with_file_read(TokioFileRead)
            .with_http_send(ReqwestHttpSend::new(client))
            .with_command_execute(TokioCommandExecute)
            .with_env(OsEnv);
        let signer = Signer::new(ctx, provider, RequestSigner::new());
        Self { signer, options }
    }

    /// Resolve an `az://` request URL to the channel URL it names and the options
    /// configured for its host.
    ///
    /// Going through [`AzureChannelUrl`] is what keeps this middleware from owning
    /// a second copy of rules that live in `rattler_azure`: that parser is what
    /// rejects userinfo, and it normalizes the authority into the exact spelling
    /// the options table is keyed by, so a grant cannot miss over case, a trailing
    /// dot, an IDNA name or an IP literal written oddly.
    ///
    /// [`AzureEndpointOptions::addressing`] is deliberately unused here: the fetch
    /// path never needs an account name, it only forwards a path. Addressing
    /// matters to the write path, which derives coordinates via
    /// `rattler_azure::account_and_container`.
    fn resolve(&self, url: &Url) -> MiddlewareResult<(AzureChannelUrl, AzureEndpointOptions)> {
        let channel = AzureChannelUrl::parse(url.as_str()).map_err(|e| {
            // The URL is not echoed back: the one rejection a user hits here is
            // userinfo, and quoting it would print their password.
            reqwest_middleware::Error::Middleware(anyhow::Error::from(e))
        })?;
        let options = self
            .options
            .get(channel.host())
            .copied()
            .unwrap_or_default();
        Ok((channel, options))
    }

    /// Whether the URL already carries an explicit SAS token (a `sig` query
    /// parameter). Such a URL is self-authenticating and must not be re-signed.
    fn has_sas_token(url: &Url) -> bool {
        url.query_pairs().any(|(key, _)| key == "sig")
    }

    /// Sign a reqwest `Request` in place using reqsign, when `auth` grants it.
    ///
    /// Two cases return without invoking reqsign at all:
    /// - The URL already carries an explicit SAS (`?...&sig=...`). Signing would
    ///   add an `Authorization` header that Azure prefers over the SAS, silently
    ///   overriding the caller's explicit token.
    /// - [`Auth::Anonymous`] — no grant. Crucially the credential is not *resolved*
    ///   either: reqsign would otherwise probe the managed-identity / IMDS endpoint
    ///   and block until it times out (~30s on a machine with no metadata service)
    ///   before we could decide not to use the result, making every anonymous
    ///   public-channel read pay that timeout — and it would pull an ambient
    ///   credential into memory for a host the user never granted.
    ///
    /// Under [`Auth::DefaultChain`] any signing failure is propagated. reqsign
    /// collapses "no credential" and "broken credential" into the same
    /// [`reqsign_core::ErrorKind::CredentialInvalid`], and since the user asked for
    /// signing there is no case left where going anonymous is the right answer.
    async fn sign(&self, req: &mut Request, auth: Auth) -> MiddlewareResult<()> {
        if Self::has_sas_token(req.url()) {
            return Ok(());
        }

        if !req.headers().contains_key("x-ms-version") {
            req.headers_mut()
                .insert("x-ms-version", http::HeaderValue::from_static(X_MS_VERSION));
        }

        match auth {
            Auth::Anonymous => {
                // The authority, not `host_str()`: a message naming a host the user
                // could act on must carry the port, or it names a host that is not
                // the one in their config.
                tracing::debug!(
                    "no `azure-options` auth grant for `{}`; sending `az://` request unsigned",
                    req.url().authority()
                );
                return Ok(());
            }
            Auth::DefaultChain => {}
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

        let (channel, options) = self.resolve(req.url())?;
        *req.url_mut() = channel.wire(options.scheme);
        self.sign(&mut req, options.auth).await?;

        let response = next.run(req, extensions).await?;

        // Azure answers an unauthorized read of a private container with 404, not
        // 403, so "no grant" and "no such blob" are the same status on the wire.
        // Say so once, naming the config the user would have to write — spelled
        // through `AzureHost` so the key printed is the key a lookup arrives with.
        if response.status() == http::StatusCode::NOT_FOUND && !options.auth.is_granted() {
            // One line, and spelled the way `AzureUrlError::InvalidHost` spells its
            // fix: a wrapped multi-line hint is harder to grep out of a log, and
            // the two guided messages should read as the same instruction.
            tracing::warn!(
                "`{}` returned 404 and this host has no `azure-options` auth grant. Azure answers \
                 an anonymous read of a *private* container with 404 rather than 403, so a missing \
                 grant looks exactly like a missing file. If the container is private, grant it in \
                 your user configuration with `[azure-options.\"{}\"]` and `auth = true`.",
                channel.canonical(),
                channel.host()
            );
        }

        Ok(response)
    }
}

#[cfg(test)]
mod tests {
    use rattler_azure::AzureScheme;

    use super::*;

    /// The `azure-options` table for one host, as a caller would build it.
    fn options(
        authority: &str,
        options: AzureEndpointOptions,
    ) -> HashMap<AzureHost, AzureEndpointOptions> {
        HashMap::from([(AzureHost::parse(authority).expect("test host"), options)])
    }

    /// A grant with everything else defaulted: anonymous is the only interesting
    /// axis in most of these tests.
    fn granted() -> AzureEndpointOptions {
        AzureEndpointOptions {
            auth: Auth::DefaultChain,
            ..Default::default()
        }
    }

    fn middleware(options: HashMap<AzureHost, AzureEndpointOptions>) -> AzureMiddleware {
        AzureMiddleware::new(Client::new(), options)
    }

    /// Resolve a URL and hand back the wire spelling its options ask for.
    fn wire_of(middleware: &AzureMiddleware, url: &str) -> String {
        let (channel, options) = middleware
            .resolve(&Url::parse(url).expect("test url"))
            .expect("url should resolve");
        channel.wire(options.scheme).to_string()
    }

    /// With no entry the scheme defaults to https, and path, query and fragment
    /// survive the rewrite untouched.
    #[test]
    fn rewrites_to_https_without_an_entry() {
        let middleware = middleware(HashMap::new());
        assert_eq!(
            wire_of(
                &middleware,
                "az://myacct.blob.core.windows.net/mychannel/noarch/repodata.json"
            ),
            "https://myacct.blob.core.windows.net/mychannel/noarch/repodata.json"
        );
        assert_eq!(
            wire_of(
                &middleware,
                "az://acct.blob.core.windows.net/c/x.json?sv=2021&sig=abc#frag"
            ),
            "https://acct.blob.core.windows.net/c/x.json?sv=2021&sig=abc#frag"
        );
    }

    /// An emulator entry is the only thing that can send an `az://` URL in
    /// cleartext, and the port has to survive — `:10000` is not any scheme's
    /// default, but `:443` would be under https and must not be dropped either.
    #[test]
    fn rewrites_to_http_for_an_emulator_entry() {
        let emulator = middleware(options(
            "127.0.0.1:10000",
            AzureEndpointOptions {
                auth: Auth::DefaultChain,
                scheme: AzureScheme::Http,
                addressing: rattler_azure::Addressing::PathStyle,
            },
        ));
        assert_eq!(
            wire_of(
                &emulator,
                "az://127.0.0.1:10000/devstoreaccount1/noarch/repodata.json"
            ),
            "http://127.0.0.1:10000/devstoreaccount1/noarch/repodata.json"
        );

        // The same host with no entry stays on https: an emulator grant must not
        // generalize to a scheme downgrade for anyone else.
        assert_eq!(
            wire_of(
                &middleware(HashMap::new()),
                "az://127.0.0.1:10000/devstoreaccount1/noarch/repodata.json"
            ),
            "https://127.0.0.1:10000/devstoreaccount1/noarch/repodata.json"
        );
    }

    /// A grant written in any spelling of a host must apply to a request for that
    /// host: a silent miss reads as a 404, i.e. "not found" for what is really
    /// "not authorized". Delegating to `AzureHost` on both sides is what buys this.
    #[test]
    fn a_grant_applies_regardless_of_how_the_host_is_spelled() {
        let middleware = middleware(options("MyCompany.blob.core.windows.net.", granted()));
        let (_, resolved) = middleware
            .resolve(&Url::parse("az://mycompany.blob.core.windows.net/c/x.json").unwrap())
            .unwrap();
        assert!(resolved.auth.is_granted());
    }

    /// Userinfo is refused before any rewrite or signing: the host is the request
    /// target verbatim, so `user:pass@real.host` can hide the real authority.
    /// (Rejection lives in `AzureHost::parse`, so there is one copy of the rule.)
    #[test]
    fn rejects_userinfo() {
        let middleware = middleware(HashMap::new());
        for url in [
            "az://user:pass@acct.blob.core.windows.net/c/x.json",
            "az://user@acct.blob.core.windows.net/c/x.json",
        ] {
            let err = middleware
                .resolve(&Url::parse(url).unwrap())
                .expect_err("userinfo must be refused");
            assert!(err.to_string().contains("userinfo"), "{err}");
        }
        assert!(
            middleware
                .resolve(&Url::parse("az://acct.blob.core.windows.net/c/x.json").unwrap())
                .is_ok()
        );
    }

    #[tokio::test]
    async fn passes_through_non_az_schemes_unchanged() {
        use reqwest_middleware::ClientBuilder;
        let client = ClientBuilder::new(Client::new())
            .with(middleware(HashMap::new()))
            .build();
        // A non-`az` request must not be rewritten; it should be attempted as-is
        // (and fail on DNS), proving the middleware left it untouched.
        let result = client
            .get("https://this-host-does-not-exist.invalid/x")
            .send()
            .await;
        assert!(result.is_err());
    }

    /// A host with no grant is sent unsigned, and its credential is never even
    /// resolved — so nothing blocks on the IMDS probe and no ambient credential is
    /// pulled into memory for a host the user never named. The provider flips a
    /// flag if it is ever asked.
    #[tokio::test]
    async fn an_ungranted_host_sends_unsigned_without_resolving_a_credential() {
        use std::sync::{
            Arc,
            atomic::{AtomicBool, Ordering},
        };

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
            HashMap::new(),
        );
        let mut req = Client::new()
            .get("https://acct.blob.core.windows.net/pub/noarch/repodata.json")
            .build()
            .unwrap();

        middleware
            .sign(&mut req, Auth::Anonymous)
            .await
            .expect("an ungranted request must pass through unsigned");

        assert!(
            !probed.load(Ordering::SeqCst),
            "credential provider must not be probed without a grant"
        );
        assert!(
            req.headers().get(http::header::AUTHORIZATION).is_none(),
            "unsigned request must not carry an Authorization header"
        );
        assert!(
            !req.url().query_pairs().any(|(k, _)| k == "sig"),
            "unsigned request must not gain a SAS query parameter"
        );
    }

    /// A granted host is actually signed: the credential resolves and the request
    /// comes back carrying Shared Key authorization.
    #[tokio::test]
    async fn a_granted_host_is_signed() {
        use reqsign_azure_storage::StaticCredentialProvider;

        let middleware = AzureMiddleware::with_credential_provider(
            Client::new(),
            // A valid base64 account key, so the provider yields a usable
            // SharedKey credential.
            StaticCredentialProvider::new_shared_key("acct", "dGVzdF9rZXk="),
            options("acct.blob.core.windows.net", granted()),
        );
        let mut req = Client::new()
            .get("https://acct.blob.core.windows.net/c/noarch/repodata.json")
            .build()
            .unwrap();

        middleware.sign(&mut req, Auth::DefaultChain).await.unwrap();

        let authorization = req
            .headers()
            .get(http::header::AUTHORIZATION)
            .expect("a granted host must be signed");
        assert!(
            authorization.to_str().unwrap().starts_with("SharedKey "),
            "{authorization:?}"
        );
    }

    /// The inversion this design turns on: with a grant, an unusable credential is
    /// a hard error. It must never degrade to an anonymous request, which Azure
    /// would answer with a 404 the user has no way to read as "auth failed". An
    /// empty provider chain resolves nothing, which reqsign reports the same way it
    /// reports a broken credential.
    #[tokio::test]
    async fn a_granted_host_with_broken_credentials_is_a_hard_error() {
        use reqsign_core::ProvideCredentialChain;

        let middleware = AzureMiddleware::with_credential_provider(
            Client::new(),
            ProvideCredentialChain::<Credential>::new(),
            options("acct.blob.core.windows.net", granted()),
        );
        let mut req = Client::new()
            .get("https://acct.blob.core.windows.net/c/noarch/repodata.json")
            .build()
            .unwrap();

        let result = middleware.sign(&mut req, Auth::DefaultChain).await;

        assert!(
            result.is_err(),
            "a granted-but-failing credential must be a hard error, not unsigned"
        );
        assert!(
            req.headers().get(http::header::AUTHORIZATION).is_none(),
            "a failed signing attempt must not leave a partial Authorization header"
        );
    }

    /// A URL that already carries a SAS token must not be re-signed even where the
    /// host is granted: Azure prefers an `Authorization` header over the SAS, so
    /// signing would silently override the caller's explicit token.
    #[tokio::test]
    async fn a_sas_in_the_url_passes_through() {
        use reqsign_azure_storage::StaticCredentialProvider;

        let middleware = AzureMiddleware::with_credential_provider(
            Client::new(),
            StaticCredentialProvider::new_shared_key("acct", "dGVzdF9rZXk="),
            options("acct.blob.core.windows.net", granted()),
        );
        let mut req = Client::new()
            .get("https://acct.blob.core.windows.net/c/x.json?sv=2021&sig=abc")
            .build()
            .unwrap();

        middleware.sign(&mut req, Auth::DefaultChain).await.unwrap();

        assert!(
            req.headers().get(http::header::AUTHORIZATION).is_none(),
            "a URL carrying an explicit SAS must not be re-signed"
        );
        assert!(
            !req.headers().contains_key("x-ms-version"),
            "a self-authenticating SAS URL is left untouched"
        );
        assert_eq!(
            req.url().as_str(),
            "https://acct.blob.core.windows.net/c/x.json?sv=2021&sig=abc"
        );
    }

    /// Serve 404 for everything, over http on localhost, standing in for a private
    /// container answering an anonymous read.
    async fn spawn_404_server() -> AzureHost {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let router = axum::Router::new().fallback(axum::http::StatusCode::NOT_FOUND);
        tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
        AzureHost::parse(&addr.to_string()).unwrap()
    }

    /// An emulator-shaped entry (http, path-style) with the grant taken from the
    /// caller, so one server can exercise both sides of the hint.
    fn emulator_entry(auth: Auth) -> AzureEndpointOptions {
        AzureEndpointOptions {
            auth,
            scheme: AzureScheme::Http,
            addressing: rattler_azure::Addressing::PathStyle,
        }
    }

    async fn get_az(middleware: AzureMiddleware, host: &AzureHost) -> reqwest::StatusCode {
        reqwest_middleware::ClientBuilder::new(Client::new())
            .with(middleware)
            .build()
            .get(format!(
                "az://{host}/devstoreaccount1/c/noarch/repodata.json"
            ))
            .send()
            .await
            .expect("request through azure middleware failed")
            .status()
    }

    /// The 404 hint must name the config block to add, keyed exactly as the table
    /// is keyed — including the port, which an earlier version of this hint
    /// dropped, printing a key that could never match.
    #[tokio::test]
    #[tracing_test::traced_test]
    async fn the_404_hint_names_the_config_block_for_an_ungranted_host() {
        let host = spawn_404_server().await;
        let middleware = middleware(options(&host.to_string(), emulator_entry(Auth::Anonymous)));

        assert_eq!(get_az(middleware, &host).await, 404);

        assert!(logs_contain(&format!("[azure-options.\"{host}\"]")));
        assert!(logs_contain("auth = true"));
    }

    /// With a grant in place a 404 means what it says, so the hint would be noise.
    #[tokio::test]
    #[tracing_test::traced_test]
    async fn the_404_hint_is_silent_for_a_granted_host() {
        use reqsign_azure_storage::StaticCredentialProvider;

        let host = spawn_404_server().await;
        let middleware = AzureMiddleware::with_credential_provider(
            Client::new(),
            StaticCredentialProvider::new_shared_key("devstoreaccount1", "dGVzdF9rZXk="),
            options(&host.to_string(), emulator_entry(Auth::DefaultChain)),
        );

        assert_eq!(get_az(middleware, &host).await, 404);

        assert!(!logs_contain("azure-options"));
    }
}
