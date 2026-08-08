//! Middleware to handle `az://` URLs to pull artifacts from Azure Blob Storage.
use std::collections::HashMap;

use async_trait::async_trait;
use rattler_azure::{
    Auth, AzureChannelUrl, AzureEndpointOptions, AzureFetchOptions, AzureHost, ContainerName,
};
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
/// A credential attaches to a request only because the user's `azure-options`
/// table grants it to the *container* the request addresses:
///
/// ```toml
/// [azure-options."mycompany.blob.core.windows.net".auth]
/// releases = true
/// # a container not listed is fetched anonymously, so one account can hold
/// # private and anonymous-read containers side by side — which is what Azure's
/// # per-container RBAC actually enforces.
///
/// [azure-options."127.0.0.1:10000"]   # Azurite
/// scheme = "http"
/// path-style = true
///
/// [azure-options."127.0.0.1:10000".auth]
/// general = true
/// ```
///
/// There is no host-level grant, by design: a single field meaning "every
/// container on this account, including the ones created later" is exactly the
/// mistake worth making unrepresentable.
///
/// Three consequences of the grant being explicit:
///
/// - **Nothing is inferred from the host name.** There is no allow-list of
///   "official" Azure suffixes, and none is needed: a host nobody granted gets
///   nothing regardless of what it is called, and an entry for a custom host *is*
///   the declaration that the user trusts that endpoint.
/// - **A broken credential is a hard error.** Because the user asked for signing,
///   an unusable credential must be reported, not silently downgraded to an
///   anonymous request that Azure will answer with a confusing 404.
/// - **A new private container fails closed** until someone adds a line for it.
///   That is the deliberate cost of a per-container grant, and it is why the 404
///   hint below prints the exact line to write: unhelped, the failure reads as "the
///   channel is broken".
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

    /// Whole `azure-options` entries, keyed by the same normalized authority the
    /// config table is keyed by. An absent host is *defined* to behave as a
    /// defaulted entry (no grants, https), so a miss is never a separate code path.
    ///
    /// Entries and not the narrower [`AzureFetchOptions`], because resolving a
    /// grant is two steps and they are ordered: the host's addressing decides which
    /// path segment is the container, and only then can the container's grant be
    /// read. The narrowing therefore happens per request, in [`Self::resolve`].
    ///
    /// A plain `HashMap` rather than `rattler_config::AzureOptionsMap`, mirroring
    /// [`crate::S3Middleware`]: taking the config type would put a mandatory
    /// `rattler_config` edge on the `azure` feature. The constructors take any
    /// iterator of host/entry pairs instead, which `AzureOptionsMap` yields
    /// directly from its own `endpoint_options`.
    options: HashMap<AzureHost, AzureEndpointOptions>,
}

/// One `az://` request, resolved against the options table.
///
/// The container is kept next to the grant it produced, because the message the
/// user needs when a request comes back 404 is the TOML line naming *that*
/// container — a hint naming only the host would be a line that grants the wrong
/// thing.
#[derive(Debug)]
struct Resolved {
    /// The channel URL the request names.
    channel: AzureChannelUrl,

    /// The container it addresses, when it addresses one. `None` for a URL with no
    /// container segment — the host root, or a path too short for the host's
    /// addressing — which has nothing to attribute a grant to.
    container: Option<ContainerName>,

    /// The grant for that container, and the wire scheme for the host.
    options: AzureFetchOptions,
}

/// What a resolved request asks of the signer.
///
/// [`Signing::Granted`] carries the container whose entry granted it, so "sign
/// this, but for no container" is unrepresentable and the failure message can
/// always quote the line that asked for signing. A grant is only ever read out of a
/// container's entry in an `auth` table, so there is no other way for one to exist.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Signing<'a> {
    /// No grant: send unsigned, and resolve no credential.
    Anonymous,

    /// `container` is granted, so sign — and fail loudly if that is impossible.
    Granted(&'a ContainerName),
}

impl<'a> Signing<'a> {
    /// The signing decision for a resolved request.
    fn new(auth: Auth, container: Option<&'a ContainerName>) -> Self {
        match (auth, container) {
            (Auth::DefaultChain, Some(container)) => Self::Granted(container),
            // `DefaultChain` without a container cannot arise — the grant was read
            // out of a container's entry — and anonymous is the arm that sends
            // nothing, which is the right way for an impossible pair to fall.
            (Auth::DefaultChain | Auth::Anonymous, _) => Self::Anonymous,
        }
    }
}

impl AzureMiddleware {
    /// Create a new Azure middleware.
    ///
    /// `client` is used for reqsign's credential resolution (IMDS / managed
    /// identity / AAD token fetches), so it must be the caller's configured
    /// client — proxy, CA bundle, and TLS settings carry through to those
    /// requests.
    ///
    /// `options` is the `azure-options` table: the per-host entries carrying the
    /// per-container grants, in any shape that iterates them —
    /// `rattler_config::AzureOptionsMap::endpoint_options` yields exactly this. An
    /// empty iterator means every `az://` request is anonymous.
    pub fn new(
        client: Client,
        options: impl IntoIterator<Item = (AzureHost, AzureEndpointOptions)>,
    ) -> Self {
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
        options: impl IntoIterator<Item = (AzureHost, AzureEndpointOptions)>,
    ) -> Self {
        let ctx = Context::new()
            .with_file_read(TokioFileRead)
            .with_http_send(ReqwestHttpSend::new(client))
            .with_command_execute(TokioCommandExecute)
            .with_env(OsEnv);
        let signer = Signer::new(ctx, provider, RequestSigner::new());
        Self {
            signer,
            options: options.into_iter().collect(),
        }
    }

    /// Resolve an `az://` request URL to the channel URL it names, the container it
    /// addresses, and the options that apply to it.
    ///
    /// Going through [`AzureChannelUrl`] is what keeps this middleware from owning
    /// a second copy of rules that live in `rattler_azure`: that parser is what
    /// rejects userinfo, and it normalizes the authority into the exact spelling
    /// the options table is keyed by, so a grant cannot miss over case, a trailing
    /// dot, an IDNA name or an IP literal written oddly. The container comes from
    /// [`rattler_azure::container`] for the same reason — it is the same derivation
    /// the write path's coordinates use, and two derivations that disagreed would
    /// look a grant up for one container and send it to another.
    ///
    /// The order is forced: the host's entry carries the addressing, the addressing
    /// says which path segment is the container, and the container selects the
    /// grant. Nothing earlier can know the container.
    fn resolve(&self, url: &Url) -> MiddlewareResult<Resolved> {
        let channel = AzureChannelUrl::parse(url.as_str()).map_err(|e| {
            // The URL is not echoed back: the one rejection a user hits here is
            // userinfo, and quoting it would print their password.
            reqwest_middleware::Error::Middleware(anyhow::Error::from(e))
        })?;

        // An absent host is defined to behave as a defaulted entry, so the fallback
        // is a value and not a branch.
        let unconfigured = AzureEndpointOptions::default();
        let entry = self.options.get(channel.host()).unwrap_or(&unconfigured);

        // A URL with no container segment resolves to no grant; a container segment
        // Azure could never accept is a malformed endpoint, and saying so beats an
        // anonymous request that comes back as an unexplained 401.
        let container = rattler_azure::container(&channel, entry.endpoint().addressing)
            .map_err(|e| reqwest_middleware::Error::Middleware(anyhow::Error::from(e)))?;

        Ok(Resolved {
            options: entry.fetch(container.as_ref()),
            container,
            channel,
        })
    }

    /// Whether the URL already carries an explicit SAS token (a `sig` query
    /// parameter). Such a URL is self-authenticating and must not be re-signed.
    fn has_sas_token(url: &Url) -> bool {
        url.query_pairs().any(|(key, _)| key == "sig")
    }

    /// Sign a reqwest `Request` in place using reqsign, when `signing` grants it.
    ///
    /// Two cases return without invoking reqsign at all:
    /// - The URL already carries an explicit SAS (`?...&sig=...`). Signing would
    ///   add an `Authorization` header that Azure prefers over the SAS, silently
    ///   overriding the caller's explicit token.
    /// - [`Signing::Anonymous`] — no grant. Crucially the credential is not *resolved*
    ///   either: reqsign would otherwise probe the managed-identity / IMDS endpoint
    ///   and block until it times out (~30s on a machine with no metadata service)
    ///   before we could decide not to use the result, making every anonymous
    ///   public-channel read pay that timeout — and it would pull an ambient
    ///   credential into memory for a host the user never granted.
    ///
    /// Under [`Signing::Granted`] any signing failure is propagated, carrying the
    /// host, the container whose grant required signing and the remedies. reqsign
    /// collapses "no
    /// credential" and "broken credential" into the same
    /// [`reqsign_core::ErrorKind::CredentialInvalid`], and since the user asked for
    /// signing there is no case left where going anonymous is the right answer.
    async fn sign(&self, req: &mut Request, signing: Signing<'_>) -> MiddlewareResult<()> {
        if Self::has_sas_token(req.url()) {
            return Ok(());
        }

        if !req.headers().contains_key("x-ms-version") {
            req.headers_mut()
                .insert("x-ms-version", http::HeaderValue::from_static(X_MS_VERSION));
        }

        let container = match signing {
            Signing::Anonymous => {
                // The authority, not `host_str()`: a message naming a host the user
                // could act on must carry the port, or it names a host that is not
                // the one in their config.
                tracing::debug!(
                    "no `azure-options` auth grant for `{}`; sending `az://` request unsigned",
                    req.url().authority()
                );
                return Ok(());
            }
            Signing::Granted(container) => container,
        };

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

        // reqsign says only "failed to load signing credential": its chain walks
        // past a provider that errors exactly as it walks past one that finds
        // nothing, so an expired `az login` and an empty environment arrive here
        // indistinguishable, after however long the chain took to give up. The host
        // and the grant that asked for signing are both in scope here and nowhere
        // further up, so this is where they get attached.
        self.signer.sign(&mut parts, None).await.map_err(|e| {
            let authority = req.url().authority();
            reqwest_middleware::Error::Middleware(anyhow::anyhow!(
                "could not resolve an Azure credential for `{container}` on `{authority}`, which \
                 `[azure-options.\"{authority}\".auth]` `{container} = true` requires: {e}\n\
                 \n\
                 Try one of:\n\
                 \x20 - `az login`\n\
                 \x20 - `AZURE_STORAGE_ACCOUNT_NAME` and `AZURE_STORAGE_ACCOUNT_KEY` in the \
                 environment\n\
                 \x20 - set `{container} = false` to fetch this container anonymously\n\
                 \n\
                 Debug logging lists the credential providers that were tried."
            ))
        })?;

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

        let Resolved {
            channel,
            container,
            options,
        } = self.resolve(req.url())?;
        *req.url_mut() = channel.wire(options.scheme);
        self.sign(&mut req, Signing::new(options.auth, container.as_ref()))
            .await?;

        let response = next.run(req, extensions).await?;

        // Azure answers an unauthorized read of a private container with 404, not
        // 403, so "no grant" and "no such blob" are the same status on the wire.
        // Under a per-container grant a newly-created private container fails closed
        // until someone writes a line for it, so this hint is what stands between
        // that and a user reading "404" as "the channel is broken". Say it once per
        // container, naming the config to write — spelled through `AzureHost` and
        // `ContainerName` so the key printed is the key a lookup arrives with.
        //
        // A URL naming no container gets no hint: there is no line that would grant
        // it anything.
        if let Some(container) = container.filter(|_| {
            response.status() == http::StatusCode::NOT_FOUND && !options.auth.is_granted()
        }) && first_404_for_container(channel.host(), &container)
        {
            // One line, and spelled the way `AzureUrlError::InvalidHost` spells its
            // fix: a wrapped multi-line hint is harder to grep out of a log, and
            // the two guided messages should read as the same instruction.
            tracing::warn!(
                "`{}` returned 404 and container `{container}` has no `azure-options` auth grant. \
                 Azure answers an anonymous read of a *private* container with 404 rather than \
                 403, so a missing grant looks exactly like a missing file. If the container is \
                 private, grant it in your user configuration with \
                 `[azure-options.\"{}\".auth]` and `{container} = true`.",
                channel.canonical(),
                channel.host()
            );
        }

        Ok(response)
    }
}

/// Whether this container still owes the 404 hint, claiming it if so.
///
/// A 404 is the *normal* answer to plenty of requests a healthy public channel
/// makes — the repodata gateway probes for a shard index under every subdir it
/// fetches, and a non-sharded channel misses every time — so a hint emitted per
/// response is a security warning printed repeatedly at users whose channel is
/// fine. Once per container per process is enough for the one case it is about: a
/// private container the user forgot to grant. Per container and not per host,
/// because the line to add differs per container: silencing a host after its first
/// ungranted container would leave the second one unexplained.
fn first_404_for_container(host: &AzureHost, container: &ContainerName) -> bool {
    static HINTED: std::sync::LazyLock<
        std::sync::Mutex<std::collections::HashSet<(AzureHost, ContainerName)>>,
    > = std::sync::LazyLock::new(Default::default);
    HINTED
        .lock()
        .expect("the 404-hint set is never held across a panic")
        .insert((host.clone(), container.clone()))
}

#[cfg(test)]
mod tests {
    use rattler_azure::{Addressing, AzureEndpoint, AzureScheme};

    use super::*;

    fn container(name: &str) -> ContainerName {
        ContainerName::new(name).expect("test container name")
    }

    /// The `azure-options` table for one host, as a caller would build it.
    fn options(
        authority: &str,
        options: AzureEndpointOptions,
    ) -> HashMap<AzureHost, AzureEndpointOptions> {
        HashMap::from([(AzureHost::parse(authority).expect("test host"), options)])
    }

    /// An entry granting one container, with everything else defaulted: which
    /// container is granted is the only interesting axis in most of these tests.
    fn granting(container_name: &str) -> AzureEndpointOptions {
        AzureEndpointOptions::new(
            [(container(container_name), Auth::DefaultChain)],
            AzureEndpoint::default(),
        )
    }

    fn middleware(options: HashMap<AzureHost, AzureEndpointOptions>) -> AzureMiddleware {
        AzureMiddleware::new(Client::new(), options)
    }

    /// Resolve a URL and hand back the wire spelling its options ask for.
    fn wire_of(middleware: &AzureMiddleware, url: &str) -> String {
        let resolved = middleware
            .resolve(&Url::parse(url).expect("test url"))
            .expect("url should resolve");
        resolved.channel.wire(resolved.options.scheme).to_string()
    }

    /// Resolve a URL, or panic with the middleware's rejection.
    fn resolve(middleware: &AzureMiddleware, url: &str) -> Resolved {
        middleware
            .resolve(&Url::parse(url).expect("test url"))
            .unwrap_or_else(|err| panic!("{url} should resolve: {err}"))
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
                "az://acct.blob.core.windows.net/general/x.json?sv=2021&sig=abc#frag"
            ),
            "https://acct.blob.core.windows.net/general/x.json?sv=2021&sig=abc#frag"
        );
    }

    /// An emulator entry is the only thing that can send an `az://` URL in
    /// cleartext, and the port has to survive — `:10000` is not any scheme's
    /// default, but `:443` would be under https and must not be dropped either.
    #[test]
    fn rewrites_to_http_for_an_emulator_entry() {
        let emulator = middleware(options("127.0.0.1:10000", emulator_entry(["general"])));
        assert_eq!(
            wire_of(
                &emulator,
                "az://127.0.0.1:10000/devstoreaccount1/general/noarch/repodata.json"
            ),
            "http://127.0.0.1:10000/devstoreaccount1/general/noarch/repodata.json"
        );

        // The same host with no entry stays on https: an emulator grant must not
        // generalize to a scheme downgrade for anyone else.
        assert_eq!(
            wire_of(
                &middleware(HashMap::new()),
                "az://127.0.0.1:10000/devstoreaccount1/general/noarch/repodata.json"
            ),
            "https://127.0.0.1:10000/devstoreaccount1/general/noarch/repodata.json"
        );
    }

    /// A grant written in any spelling of a host must apply to a request for that
    /// host: a silent miss reads as a 404, i.e. "not found" for what is really
    /// "not authorized". Delegating to `AzureHost` on both sides is what buys this.
    #[test]
    fn a_grant_applies_regardless_of_how_the_host_is_spelled() {
        let middleware = middleware(options(
            "MyCompany.blob.core.windows.net.",
            granting("releases"),
        ));
        assert!(
            resolve(
                &middleware,
                "az://mycompany.blob.core.windows.net/releases/x.json"
            )
            .options
            .auth
            .is_granted()
        );
    }

    /// The point of the per-container table: one account holding a private and an
    /// anonymous-read container is configurable, because the grant stops at the
    /// container it names. Under a host-level grant the second URL here would be
    /// signed too, and 403 for any identity holding no role on it.
    #[test]
    fn a_grant_stops_at_the_container_it_names() {
        let middleware = middleware(options(
            "mycompany.blob.core.windows.net",
            AzureEndpointOptions::new(
                [
                    (container("releases"), Auth::DefaultChain),
                    // Redundant with omission, and legal: it says "deliberately
                    // unsigned" rather than "forgotten".
                    (container("public"), Auth::Anonymous),
                ],
                AzureEndpoint::default(),
            ),
        ));

        for (url, granted) in [
            ("az://mycompany.blob.core.windows.net/releases/x.json", true),
            ("az://mycompany.blob.core.windows.net/public/x.json", false),
            ("az://mycompany.blob.core.windows.net/staging/x.json", false),
        ] {
            assert_eq!(
                resolve(&middleware, url).options.auth.is_granted(),
                granted,
                "{url}"
            );
        }
    }

    /// A container is found where the host's addressing says it is, so a grant on a
    /// path-style host applies to the second segment and not the account in the
    /// first.
    #[test]
    fn a_container_is_read_through_the_hosts_addressing() {
        let path_style = middleware(options("127.0.0.1:10000", emulator_entry(["general"])));
        let resolved = resolve(
            &path_style,
            "az://127.0.0.1:10000/devstoreaccount1/general/noarch/repodata.json",
        );
        assert_eq!(resolved.container, Some(container("general")));
        assert!(resolved.options.auth.is_granted());

        // Host-style on the same URL reads the account segment as the container, so
        // the grant does not apply — the addressing is what decides which name a
        // grant is even about.
        let host_style = middleware(options(
            "127.0.0.1:10000",
            AzureEndpointOptions::new(
                [(container("general"), Auth::DefaultChain)],
                AzureEndpoint {
                    scheme: AzureScheme::Http,
                    addressing: Addressing::HostStyle,
                },
            ),
        ));
        let resolved = resolve(
            &host_style,
            "az://127.0.0.1:10000/devstoreaccount1/general/noarch/repodata.json",
        );
        assert_eq!(resolved.container, Some(container("devstoreaccount1")));
        assert!(!resolved.options.auth.is_granted());
    }

    /// A URL naming no container has nothing to attribute a grant to, so it is
    /// anonymous rather than an error: the fetch path stays total for URLs that are
    /// not channel-scoped, which is what it is today.
    #[test]
    fn a_url_without_a_container_is_anonymous() {
        let middleware = middleware(options(
            "mycompany.blob.core.windows.net",
            granting("releases"),
        ));

        for url in [
            "az://mycompany.blob.core.windows.net",
            "az://mycompany.blob.core.windows.net/",
            "az://mycompany.blob.core.windows.net/?comp=list",
        ] {
            let resolved = resolve(&middleware, url);
            assert_eq!(resolved.container, None, "{url}");
            assert!(!resolved.options.auth.is_granted(), "{url}");
        }
    }

    /// A container segment Azure could never accept is a malformed endpoint, not an
    /// ungranted one: no legitimate request can land here, and going quietly
    /// anonymous would surface as an unexplained 401 rather than naming the fault.
    #[test]
    fn a_url_with_an_unusable_container_is_refused() {
        let middleware = middleware(options(
            "mycompany.blob.core.windows.net",
            granting("releases"),
        ));

        for url in [
            "az://mycompany.blob.core.windows.net/Releases/x.json",
            "az://mycompany.blob.core.windows.net/ab/x.json",
        ] {
            let err = middleware
                .resolve(&Url::parse(url).unwrap())
                .expect_err("an illegal container name must be refused");
            assert!(err.to_string().contains("container name"), "{url}: {err}");
        }
    }

    /// Userinfo is refused before any rewrite or signing: the host is the request
    /// target verbatim, so `user:pass@real.host` can hide the real authority.
    /// (Rejection lives in `AzureHost::parse`, so there is one copy of the rule.)
    #[test]
    fn rejects_userinfo() {
        let middleware = middleware(HashMap::new());
        for url in [
            "az://user:pass@acct.blob.core.windows.net/general/x.json",
            "az://user@acct.blob.core.windows.net/general/x.json",
        ] {
            let err = middleware
                .resolve(&Url::parse(url).unwrap())
                .expect_err("userinfo must be refused");
            assert!(err.to_string().contains("userinfo"), "{err}");
        }
        assert!(
            middleware
                .resolve(&Url::parse("az://acct.blob.core.windows.net/general/x.json").unwrap())
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

    /// A container with no grant is sent unsigned, and its credential is never even
    /// resolved — so nothing blocks on the IMDS probe and no ambient credential is
    /// pulled into memory for a host the user never named. The provider flips a
    /// flag if it is ever asked.
    #[tokio::test]
    async fn an_ungranted_container_sends_unsigned_without_resolving_a_credential() {
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
            .sign(&mut req, Signing::Anonymous)
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

    /// A granted container is actually signed: the credential resolves and the
    /// request comes back carrying Shared Key authorization.
    #[tokio::test]
    async fn a_granted_container_is_signed() {
        use reqsign_azure_storage::StaticCredentialProvider;

        let middleware = AzureMiddleware::with_credential_provider(
            Client::new(),
            // A valid base64 account key, so the provider yields a usable
            // SharedKey credential.
            StaticCredentialProvider::new_shared_key("acct", "dGVzdF9rZXk="),
            options("acct.blob.core.windows.net", granting("releases")),
        );
        let mut req = Client::new()
            .get("https://acct.blob.core.windows.net/releases/noarch/repodata.json")
            .build()
            .unwrap();

        middleware
            .sign(&mut req, Signing::Granted(&container("releases")))
            .await
            .unwrap();

        let authorization = req
            .headers()
            .get(http::header::AUTHORIZATION)
            .expect("a granted container must be signed");
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
    async fn a_granted_container_with_broken_credentials_is_a_hard_error() {
        use reqsign_core::ProvideCredentialChain;

        let middleware = AzureMiddleware::with_credential_provider(
            Client::new(),
            ProvideCredentialChain::<Credential>::new(),
            options("acct.blob.core.windows.net", granting("releases")),
        );
        let mut req = Client::new()
            .get("https://acct.blob.core.windows.net/releases/noarch/repodata.json")
            .build()
            .unwrap();

        let result = middleware
            .sign(&mut req, Signing::Granted(&container("releases")))
            .await;

        assert!(
            result.is_err(),
            "a granted-but-failing credential must be a hard error, not unsigned"
        );
        assert!(
            req.headers().get(http::header::AUTHORIZATION).is_none(),
            "a failed signing attempt must not leave a partial Authorization header"
        );

        // reqsign's own message names neither the host nor a remedy, and the chain
        // hides which provider failed. Everything actionable has to come from here —
        // including which container's grant asked for the signing, since the entry
        // may hold several and only one line is at fault.
        let message = result.unwrap_err().to_string();
        for expected in [
            "acct.blob.core.windows.net",
            "releases = true",
            "az login",
            "AZURE_STORAGE_ACCOUNT_KEY",
        ] {
            assert!(
                message.contains(expected),
                "the failure must name `{expected}`, got: {message}"
            );
        }
    }

    /// A URL that already carries a SAS token must not be re-signed even where the
    /// container is granted: Azure prefers an `Authorization` header over the SAS,
    /// so signing would silently override the caller's explicit token.
    #[tokio::test]
    async fn a_sas_in_the_url_passes_through() {
        use reqsign_azure_storage::StaticCredentialProvider;

        let middleware = AzureMiddleware::with_credential_provider(
            Client::new(),
            StaticCredentialProvider::new_shared_key("acct", "dGVzdF9rZXk="),
            options("acct.blob.core.windows.net", granting("releases")),
        );
        let mut req = Client::new()
            .get("https://acct.blob.core.windows.net/releases/x.json?sv=2021&sig=abc")
            .build()
            .unwrap();

        middleware
            .sign(&mut req, Signing::Granted(&container("releases")))
            .await
            .unwrap();

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
            "https://acct.blob.core.windows.net/releases/x.json?sv=2021&sig=abc"
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

    /// An emulator-shaped entry (http, path-style) granting whichever containers the
    /// caller names, so one server can exercise both sides of the hint.
    fn emulator_entry<'a>(granted: impl IntoIterator<Item = &'a str>) -> AzureEndpointOptions {
        AzureEndpointOptions::new(
            granted
                .into_iter()
                .map(|name| (container(name), Auth::DefaultChain)),
            AzureEndpoint {
                scheme: AzureScheme::Http,
                addressing: Addressing::PathStyle,
            },
        )
    }

    async fn get_az(
        middleware: AzureMiddleware,
        host: &AzureHost,
        container: &str,
    ) -> reqwest::StatusCode {
        reqwest_middleware::ClientBuilder::new(Client::new())
            .with(middleware)
            .build()
            .get(format!(
                "az://{host}/devstoreaccount1/{container}/noarch/repodata.json"
            ))
            .send()
            .await
            .expect("request through azure middleware failed")
            .status()
    }

    /// The 404 hint must name the config block to add, keyed exactly as the table
    /// is keyed — including the port, which an earlier version of this hint
    /// dropped, printing a key that could never match — and the container, since
    /// that is the line the user has to write.
    #[tokio::test]
    #[tracing_test::traced_test]
    async fn the_404_hint_names_the_config_block_for_an_ungranted_container() {
        let host = spawn_404_server().await;
        let middleware = middleware(options(&host.to_string(), emulator_entry([])));

        assert_eq!(get_az(middleware, &host, "general").await, 404);

        assert!(logs_contain(&format!("[azure-options.\"{host}\".auth]")));
        assert!(logs_contain("general = true"));
    }

    /// A public non-sharded channel 404s on every shard-index probe the repodata
    /// gateway makes, so a hint per response is a security warning repeated at a
    /// user whose channel is healthy. It is silenced per container, though: a second
    /// ungranted container needs a different line, so it gets its own hint.
    #[tokio::test]
    #[tracing_test::traced_test]
    async fn the_404_hint_is_emitted_once_per_container() {
        let host = spawn_404_server().await;
        let client = reqwest_middleware::ClientBuilder::new(Client::new())
            .with(middleware(options(&host.to_string(), emulator_entry([]))))
            .build();

        for container in ["general", "staging"] {
            for subdir in ["noarch", "linux-64", "osx-64"] {
                let status = client
                    .get(format!(
                        "az://{host}/devstoreaccount1/{container}/{subdir}/\
                         repodata_shards.msgpack.zst"
                    ))
                    .send()
                    .await
                    .expect("request through azure middleware failed")
                    .status();
                assert_eq!(status, 404);
            }
        }

        for container in ["general", "staging"] {
            logs_assert(move |lines: &[&str]| {
                let hints = lines
                    .iter()
                    .filter(|line| line.contains(&format!("{container} = true")))
                    .count();
                (hints == 1).then_some(()).ok_or_else(|| {
                    format!("expected exactly one hint for {container}, got {hints}")
                })
            });
        }
    }

    /// With a grant in place a 404 means what it says, so the hint would be noise.
    #[tokio::test]
    #[tracing_test::traced_test]
    async fn the_404_hint_is_silent_for_a_granted_container() {
        use reqsign_azure_storage::StaticCredentialProvider;

        let host = spawn_404_server().await;
        let middleware = AzureMiddleware::with_credential_provider(
            Client::new(),
            StaticCredentialProvider::new_shared_key("devstoreaccount1", "dGVzdF9rZXk="),
            options(&host.to_string(), emulator_entry(["general"])),
        );

        assert_eq!(get_az(middleware, &host, "general").await, 404);

        assert!(!logs_contain("azure-options"));
    }
}
