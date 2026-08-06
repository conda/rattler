//! Middleware to handle `oci://` URLs to pull artifacts from an OCI registry
use std::{
    collections::HashMap,
    fmt::{Display, Formatter},
    sync::{Arc, Mutex},
};

use base64::{Engine, prelude::BASE64_STANDARD};
use http::{
    Extensions,
    header::{ACCEPT, AUTHORIZATION},
};
use reqwest::{Request, Response, StatusCode, header::HeaderValue};
use reqwest_middleware::{Middleware, Next};
use serde::Deserialize;
use url::{ParseError, Url};

use crate::{
    Authentication, AuthenticationStorage, Challenge, LazyClient,
    challenge_middleware::parse_challenges, mirror_middleware::create_404_response,
};

#[derive(thiserror::Error, Debug)]
enum OciMiddlewareError {
    #[error("Reqwest error: {0}")]
    Reqwest(#[from] reqwest::Error),

    #[error("Reqwest middleware error: {0}")]
    ReqwestMiddleware(#[from] reqwest_middleware::Error),

    #[error("URL parse error: {0}")]
    ParseError(#[from] ParseError),

    #[error("Invalid header value: {0}")]
    InvalidHeaderValue(#[from] reqwest::header::InvalidHeaderValue),

    #[error("Layer not found")]
    LayerNotFound,

    #[error("Manifest request failed with status {0}")]
    ManifestRequestFailed(StatusCode),

    #[error("Invalid OCI URL '{0}': {1}")]
    InvalidUrl(Url, &'static str),

    #[error("OCI registry requested authentication")]
    AuthenticationRequired(Vec<Challenge>),
}

/// Middleware to handle `oci://` URLs
///
/// Authentication follows the registry's `WWW-Authenticate` challenge: a
/// `Bearer` challenge is exchanged for a scoped token at its realm, anything
/// else sends stored credentials directly. Anonymous without
/// [`with_authentication_storage`](Self::with_authentication_storage), which is
/// enough for public registries.
#[derive(Debug, Clone)]
pub struct OciMiddleware {
    /// Shared HTTP client reused across all OCI requests to avoid creating a
    /// new connection pool on every token fetch or manifest pull.
    client: LazyClient,

    /// Credentials for private registries. Without a storage the middleware
    /// only ever accesses registries anonymously.
    auth_storage: Option<AuthenticationStorage>,

    /// What [`OciMiddleware::registry_auth`] discovered about each registry
    /// host so far, keyed by host.
    registry_auth_cache: Arc<Mutex<HashMap<String, RegistryAuth>>>,
}

impl OciMiddleware {
    /// Create a new [`OciMiddleware`] reusing the provided HTTP client.
    pub fn new(client: impl Into<LazyClient>) -> Self {
        Self {
            client: client.into(),
            auth_storage: None,
            registry_auth_cache: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// Use `storage` to look up credentials for private registries.
    #[must_use]
    pub fn with_authentication_storage(mut self, storage: AuthenticationStorage) -> Self {
        self.auth_storage = Some(storage);
        self
    }

    /// Credentials stored for the registry, if any. [`AuthenticationStorage`]
    /// resolves by host, so the `oci://` scheme is not a problem.
    async fn stored_credentials(&self, url: &Url) -> Option<Authentication> {
        match self
            .auth_storage
            .as_ref()?
            .get_by_url_refreshed(url.clone())
            .await
        {
            Ok((_, credentials)) => credentials,
            Err(e) => {
                tracing::warn!("OCI Mirror: could not look up credentials for {url}: {e}");
                None
            }
        }
    }

    /// How `host` wants to be authenticated, probing it at most once per
    /// process instead of once per artifact.
    async fn registry_auth(&self, host: &str) -> RegistryAuth {
        let cached = self
            .registry_auth_cache
            .lock()
            .expect("OCI registry auth cache poisoned")
            .get(host)
            .cloned();
        if let Some(cached) = cached {
            return cached;
        }

        // Don't cache a failed probe in case it's a temporary error.
        let Some(auth) = self.probe_registry_auth(host).await else {
            return RegistryAuth::Direct;
        };

        // Concurrent first requests to the same host can both probe it. The
        // answer is the same either way, so one duplicate request is cheaper
        // than single-flight machinery.
        self.registry_auth_cache
            .lock()
            .expect("OCI registry auth cache poisoned")
            .insert(host.to_string(), auth.clone());

        auth
    }

    /// Ask the registry how it wants to be authenticated with the OCI API
    /// version check (`GET /v2/`).
    ///
    /// `None` when the registry did not answer, which is not the same as it
    /// answering that it wants no negotiation.
    async fn probe_registry_auth(&self, host: &str) -> Option<RegistryAuth> {
        let Ok(url) = format!("https://{host}/v2/").parse::<Url>() else {
            return Some(RegistryAuth::Direct);
        };

        match self.client.client().get(url.clone()).send().await {
            Ok(response) => {
                let status = response.status();
                let challenges = parse_challenges(response.headers());
                let result = registry_auth_from_probe(status, &challenges);
                if result.is_none() {
                    // Do not let a transient response permanently poison the
                    // host-wide auth cache as `Direct`.
                    tracing::debug!(
                        "OCI Mirror: auth probe {url} returned unusable status {status} or challenge"
                    );
                }
                result
            }
            Err(e) => {
                // The artifact request that follows reports a better error than
                // we could here, so a failed probe must not be fatal.
                tracing::debug!("OCI Mirror: could not probe {url} for its auth challenge: {e}");
                None
            }
        }
    }

    /// The `Authorization` header to send for `oci_url`, honouring the
    /// registry's challenge.
    async fn authorization_header(
        &self,
        oci_url: &OCIUrl,
        action: OciAction,
    ) -> Result<Option<HeaderValue>, OciMiddlewareError> {
        let credentials = self.stored_credentials(&oci_url.url).await;

        // A stored bearer token already *is* a registry token, so there is
        // nothing to exchange it for.
        if matches!(
            credentials,
            Some(Authentication::BearerToken(_) | Authentication::OAuth { .. })
        ) {
            return Ok(credentials_header(credentials.as_ref()));
        }

        match self.registry_auth(&oci_url.host).await {
            RegistryAuth::TokenExchange { realm, service } => {
                let token_url = token_url(&realm, service.as_deref(), &oci_url.path, action);
                let token = get_token(&self.client, &token_url, credentials.as_ref()).await?;
                let mut header = HeaderValue::from_str(&format!("Bearer {token}"))?;
                header.set_sensitive(true);
                Ok(Some(header))
            }
            RegistryAuth::Direct => Ok(credentials_header(credentials.as_ref())),
        }
    }

    /// Resolve a challenge returned by a concrete manifest or blob request.
    /// Unlike the `/v2/` probe, this is authoritative for the requested
    /// repository and therefore replaces the cached host-wide answer.
    async fn authorization_from_challenges(
        &self,
        oci_url: &OCIUrl,
        challenges: &[Challenge],
    ) -> Result<Option<HeaderValue>, OciMiddlewareError> {
        let auth = registry_auth_from_challenges(challenges);
        self.registry_auth_cache
            .lock()
            .expect("OCI registry auth cache poisoned")
            .insert(oci_url.host.clone(), auth.clone());

        let credentials = self.stored_credentials(&oci_url.url).await;
        match auth {
            RegistryAuth::TokenExchange { realm, service } => {
                let token_url =
                    token_url(&realm, service.as_deref(), &oci_url.path, OciAction::Pull);
                let token = get_token(&self.client, &token_url, credentials.as_ref()).await?;
                let mut header = HeaderValue::from_str(&format!("Bearer {token}"))?;
                header.set_sensitive(true);
                Ok(Some(header))
            }
            RegistryAuth::Direct => Ok(credentials_header(credentials.as_ref())),
        }
    }

    /// Turn an `oci://` request into a request for the registry blob that holds
    /// the artifact.
    async fn rewrite_to_blob_request(
        &self,
        oci_url: &OCIUrl,
        req: &mut Request,
        expected_sha256: Option<&str>,
    ) -> Result<(), OciMiddlewareError> {
        let authorization = self.authorization_header(oci_url, OciAction::Pull).await?;
        match oci_url
            .set_blob_url(&self.client, req, authorization.as_ref(), expected_sha256)
            .await
        {
            Err(OciMiddlewareError::AuthenticationRequired(challenges)) => {
                let authorization = self
                    .authorization_from_challenges(oci_url, &challenges)
                    .await?;
                oci_url
                    .set_blob_url(&self.client, req, authorization.as_ref(), expected_sha256)
                    .await
            }
            result => result,
        }
    }

    /// Send `req` downstream, keeping one copy so a repository-specific
    /// `WWW-Authenticate` challenge can be answered and the request replayed
    /// exactly once.
    async fn run_with_challenge_retry(
        &self,
        oci_url: &OCIUrl,
        req: Request,
        extensions: &mut Extensions,
        next: &Next<'_>,
    ) -> reqwest_middleware::Result<Response> {
        let Some(mut retry_req) = req.try_clone() else {
            return next.clone().run(req, extensions).await;
        };
        let response = next.clone().run(req, extensions).await?;
        if response.status() != StatusCode::UNAUTHORIZED {
            return Ok(response);
        }

        let challenges = parse_challenges(response.headers());
        if challenges.is_empty() {
            return Ok(response);
        }
        let Some(authorization) = self
            .authorization_from_challenges(oci_url, &challenges)
            .await
            .map_err(|e| reqwest_middleware::Error::Middleware(e.into()))?
        else {
            return Ok(response);
        };
        retry_req.headers_mut().insert(AUTHORIZATION, authorization);
        next.clone().run(retry_req, extensions).await
    }
}

/// The authentication a registry asks for in its `GET /v2/` response.
#[derive(Clone, Debug, PartialEq, Eq)]
enum RegistryAuth {
    /// A `Bearer` challenge: exchange credentials for a scoped registry token
    /// at the challenge's realm (`https://ghcr.io/token` for ghcr.io).
    TokenExchange { realm: Url, service: Option<String> },

    /// Everything else: a `Basic` challenge (Amazon ECR), no challenge at all,
    /// or a scheme we don't implement. There is nothing to negotiate, so send
    /// the stored credentials as they are — or nothing, and let the registry
    /// answer with its own error.
    Direct,
}

/// Interpret the result of the best-effort `GET /v2/` probe.
///
/// Unexpected statuses and a `401` without a parseable challenge are not
/// cached: both can be transient and neither tells us how the registry wants
/// the concrete repository request authenticated.
fn registry_auth_from_probe(status: StatusCode, challenges: &[Challenge]) -> Option<RegistryAuth> {
    if status == StatusCode::UNAUTHORIZED {
        return (!challenges.is_empty()).then(|| registry_auth_from_challenges(challenges));
    }
    status
        .is_success()
        .then(|| registry_auth_from_challenges(challenges))
}

/// Pick the authentication flow from a registry's challenges.
///
/// A `Bearer` challenge whose `realm` is missing, unparsable, or not HTTPS
/// degrades to [`RegistryAuth::Direct`] rather than erroring: a registry we
/// cannot negotiate with may still accept stored credentials directly.
fn registry_auth_from_challenges(challenges: &[Challenge]) -> RegistryAuth {
    for challenge in challenges {
        if !challenge.scheme.eq_ignore_ascii_case("bearer") {
            continue;
        }
        let Some(realm) = challenge
            .params
            .get("realm")
            .and_then(|realm| Url::parse(realm).ok())
            .filter(|realm| realm.scheme() == "https")
        else {
            // Basic credentials are forwarded to this endpoint during token
            // exchange. Never allow a challenge to downgrade them to cleartext.
            tracing::debug!("OCI Mirror: ignoring Bearer challenge without a usable HTTPS realm");
            continue;
        };
        return RegistryAuth::TokenExchange {
            realm,
            service: challenge.params.get("service").cloned(),
        };
    }
    RegistryAuth::Direct
}

/// The Docker-style token exchange URL: the challenge's realm, plus the service
/// it named and the scope for this artifact.
fn token_url(realm: &Url, service: Option<&str>, path: &str, action: OciAction) -> Url {
    let mut url = realm.clone();
    let mut query = url.query_pairs_mut();
    if let Some(service) = service {
        query.append_pair("service", service);
    }
    query.append_pair("scope", &format!("repository:{path}:{action}"));
    drop(query);
    url
}

/// The `Authorization` header for stored credentials. `None` when there are
/// none, or when they are not something an HTTP header can carry: conda tokens
/// live in the URL and S3 credentials sign the request instead.
fn credentials_header(credentials: Option<&Authentication>) -> Option<HeaderValue> {
    let value = match credentials? {
        Authentication::BasicHTTP { username, password } => {
            format!(
                "Basic {}",
                BASE64_STANDARD.encode(format!("{username}:{password}"))
            )
        }
        Authentication::BearerToken(token) => format!("Bearer {token}"),
        Authentication::OAuth { access_token, .. } => format!("Bearer {access_token}"),
        Authentication::CondaToken(_) | Authentication::S3Credentials { .. } => return None,
    };

    // Never log the value itself, not even in the error case.
    let Ok(mut header) = HeaderValue::from_str(&value) else {
        tracing::warn!(
            "OCI Mirror: stored credentials are not a valid header value, continuing without them"
        );
        return None;
    };
    header.set_sensitive(true);
    Some(header)
}

/// The action to perform on the OCI registry
pub enum OciAction {
    /// Pull an artifact
    Pull,
    /// Push an artifact
    Push,
    /// Push and/or pull an artifact
    PushPull,
}

#[derive(Clone, Debug, Deserialize)]
struct OCIToken {
    token: String,
}

impl Display for OciAction {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            OciAction::Pull => write!(f, "pull"),
            OciAction::Push => write!(f, "push"),
            OciAction::PushPull => write!(f, "push,pull"),
        }
    }
}

// [oci://ghcr.io/channel-mirrors/conda-forge]/[osx-arm64/xtensor]
async fn get_token(
    client: &LazyClient,
    token_url: &Url,
    credentials: Option<&Authentication>,
) -> Result<String, OciMiddlewareError> {
    let mut request = client.client().get(token_url.clone());

    // Like Docker, present stored credentials to the token endpoint: an
    // anonymous exchange only ever yields a token for public repositories.
    if let Some(header) = credentials_header(credentials) {
        request = request.header(AUTHORIZATION, header);
    }

    let response = request.send().await?;

    match response.error_for_status() {
        Ok(response) => {
            let token = response.json::<OCIToken>().await?;
            Ok(token.token)
        }
        Err(e) => {
            tracing::error!("OCI Mirror: failed to get token with URL: {}", token_url);
            Err(OciMiddlewareError::Reqwest(e))
        }
    }
}

#[derive(Debug)]
struct OCIUrl {
    url: Url,
    host: String,
    path: String,
    tag: String,
    media_type: String,
}

/// OCI registry tags are not allowed to contain `+`, `!`, or `=`, so we need to
/// replace them with something else (reverse of `version_build_tag`)
#[allow(dead_code)]
fn reverse_version_build_tag(tag: &str) -> String {
    tag.replace("__p__", "+")
        .replace("__e__", "!")
        .replace("__eq__", "=")
}

/// OCI registry tags are not allowed to contain `+`, `!`, or `=`, so we need to
/// replace them with something else
fn version_build_tag(tag: &str) -> String {
    tag.replace('+', "__p__")
        .replace('!', "__e__")
        .replace('=', "__eq__")
}

impl OCIUrl {
    pub fn manifest_url(&self) -> Result<Url, ParseError> {
        format!(
            "https://{}/v2/{}/manifests/{}",
            self.host, self.path, self.tag
        )
        .parse()
    }

    pub fn blob_url(&self, sha256: &str) -> Result<Url, ParseError> {
        format!("https://{}/v2/{}/blobs/{}", self.host, self.path, sha256).parse()
    }

    pub fn new(url: &Url) -> Result<Self, OciMiddlewareError> {
        // get filename (last segment of path)
        let filename = url
            .path_segments()
            .and_then(|mut s| s.next_back())
            .ok_or_else(|| {
                OciMiddlewareError::InvalidUrl(url.clone(), "URL has no path segments")
            })?;

        let mut res = OCIUrl {
            url: url.clone(),
            tag: "latest".to_string(),
            media_type: "".to_string(),
            host: url.host_str().unwrap_or("").to_string(),
            path: url.path().trim_start_matches('/').to_string(),
        };

        let mut computed_filename = filename.to_string();

        // We reimplement some archive name splitting logic from rattler here
        // because we don't want to introduce cyclic dependencies
        if let Some(archive_name) = filename.strip_suffix(".conda") {
            let parts = archive_name.rsplitn(3, '-').collect::<Vec<&str>>();
            match parts.as_slice() {
                [build, version, name] => {
                    computed_filename = name.to_string();
                    res.tag = version_build_tag(&format!("{version}-{build}"));
                    res.media_type = "application/vnd.conda.package.v2".to_string();
                }
                _ => {
                    return Err(OciMiddlewareError::InvalidUrl(
                        url.clone(),
                        "package filename must have the form name-version-build.conda",
                    ));
                }
            }
        } else if let Some(archive_name) = filename.strip_suffix(".tar.bz2") {
            let parts = archive_name.rsplitn(3, '-').collect::<Vec<&str>>();
            match parts.as_slice() {
                [build, version, name] => {
                    computed_filename = name.to_string();
                    res.tag = version_build_tag(&format!("{version}-{build}"));
                    res.media_type = "application/vnd.conda.package.v1".to_string();
                }
                _ => {
                    return Err(OciMiddlewareError::InvalidUrl(
                        url.clone(),
                        "package filename must have the form name-version-build.tar.bz2",
                    ));
                }
            }
        } else if filename.starts_with("repodata.json") {
            computed_filename = "repodata.json".to_string();
            if filename == "repodata.json" {
                res.media_type = "application/vnd.conda.repodata.v1+json".to_string();
            } else if filename.ends_with(".gz") {
                res.media_type = "application/vnd.conda.repodata.v1+json+gzip".to_string();
            } else if filename.ends_with(".bz2") {
                res.media_type = "application/vnd.conda.repodata.v1+json+bz2".to_string();
            } else if filename.ends_with(".zst") {
                res.media_type = "application/vnd.conda.repodata.v1+json+zst".to_string();
            }
        }

        // OCI image names cannot start with `_`, so we prefix it with `zzz`
        if computed_filename.starts_with('_') {
            computed_filename = format!("zzz{computed_filename}");
        }

        res.url = url.join(&computed_filename)?;
        res.path = res.url.path().trim_start_matches('/').to_string();
        Ok(res)
    }

    /// Point `req` at the blob holding this artifact, authenticated with
    /// `authorization` (which the registry's challenge decided upon).
    ///
    /// With `expected_sha256` the blob is addressed directly; without it the
    /// manifest is pulled first to learn the digest.
    pub async fn set_blob_url(
        &self,
        client: &LazyClient,
        req: &mut Request,
        authorization: Option<&HeaderValue>,
        expected_sha256: Option<&str>,
    ) -> Result<(), OciMiddlewareError> {
        if let Some(header) = authorization {
            req.headers_mut().insert(AUTHORIZATION, header.clone());
        }

        // if we know the hash, we can pull the artifact directly
        // if we don't, we need to pull the manifest and then pull the artifact
        if let Some(expected_sha_hash) = expected_sha256 {
            *req.url_mut() = self.blob_url(&format!("sha256:{expected_sha_hash}"))?;
        } else {
            // get the tag from the URL retrieve the manifest
            let manifest_url = self.manifest_url()?; // TODO: handle error

            let mut manifest_request = client
                .client()
                .get(manifest_url)
                .header(ACCEPT, "application/vnd.oci.image.manifest.v1+json");
            if let Some(header) = authorization {
                manifest_request = manifest_request.header(AUTHORIZATION, header.clone());
            }

            let manifest = manifest_request.send().await?;
            if manifest.status() == StatusCode::UNAUTHORIZED {
                let challenges = parse_challenges(manifest.headers());
                if !challenges.is_empty() {
                    return Err(OciMiddlewareError::AuthenticationRequired(challenges));
                }
            }

            if !manifest.status().is_success() {
                return Err(OciMiddlewareError::ManifestRequestFailed(manifest.status()));
            }
            let manifest: Manifest = manifest.json().await?;

            let layer = if let Some(layer) = manifest
                .layers
                .iter()
                .find(|l| l.media_type == self.media_type)
            {
                layer
            } else {
                return Err(OciMiddlewareError::LayerNotFound);
            };

            *req.url_mut() = self.blob_url(&layer.digest)?;
        }

        Ok(())
    }
}

#[allow(dead_code)]
#[derive(Debug, Deserialize)]
struct Layer {
    digest: String,
    #[serde(rename = "mediaType")]
    media_type: String,
    size: u64,
    annotations: Option<HashMap<String, String>>,
}

#[allow(dead_code)]
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Manifest {
    schema_version: u64,
    layers: Vec<Layer>,
    config: Layer,
    annotations: Option<HashMap<String, String>>,
}

#[cfg_attr(not(target_arch = "wasm32"), async_trait::async_trait)]
#[cfg_attr(target_arch = "wasm32", async_trait::async_trait(?Send))]
impl Middleware for OciMiddleware {
    async fn handle(
        &self,
        mut req: Request,
        extensions: &mut Extensions,
        next: Next<'_>,
    ) -> reqwest_middleware::Result<Response> {
        // if the URL is not an OCI URL, we don't need to do anything
        if req.url().scheme() != "oci" {
            return next.run(req, extensions).await;
        }

        let oci_url = match OCIUrl::new(req.url()) {
            Ok(url) => url,
            Err(e) => return Err(reqwest_middleware::Error::Middleware(e.into())),
        };

        let expected_sha256 = req
            .headers()
            .get("X-Expected-Sha256")
            .and_then(|s| s.to_str().ok())
            .map(ToString::to_string);

        if let Err(e) = self
            .rewrite_to_blob_request(&oci_url, &mut req, expected_sha256.as_deref())
            .await
        {
            return lookup_error_to_response(e, req.url());
        }

        let fallback_req = if expected_sha256.is_some() {
            req.try_clone()
        } else {
            None
        };
        let response = self
            .run_with_challenge_retry(&oci_url, req, extensions, &next)
            .await?;

        // Pull-through caches (e.g. Amazon ECR) only import an artifact once
        // its manifest is pulled, so a digest-addressed blob can 404 while the
        // manifest route still works. Retry through the manifest once.
        let Some(mut fallback_req) = fallback_req else {
            return Ok(response);
        };
        if response.status() != StatusCode::NOT_FOUND {
            return Ok(response);
        }
        if let Err(e) = self
            .rewrite_to_blob_request(&oci_url, &mut fallback_req, None)
            .await
        {
            return lookup_error_to_response(e, fallback_req.url());
        }
        self.run_with_challenge_retry(&oci_url, fallback_req, extensions, &next)
            .await
    }
}

/// Determine whether a lookup error is a safe 404 or a fatal error.
fn lookup_error_to_response(
    error: OciMiddlewareError,
    url: &Url,
) -> reqwest_middleware::Result<Response> {
    match error {
        OciMiddlewareError::LayerNotFound => Ok(create_404_response(
            url,
            "No layer available for media type",
        )),
        OciMiddlewareError::ManifestRequestFailed(StatusCode::NOT_FOUND) => {
            Ok(create_404_response(url, "Manifest not found"))
        }
        _ => Err(reqwest_middleware::Error::Middleware(error.into())),
    }
}

#[cfg(test)]
mod tests {
    use sha2::{Digest, Sha256};

    use super::{
        Authentication, OciAction, RegistryAuth, StatusCode, credentials_header, parse_challenges,
        registry_auth_from_challenges, registry_auth_from_probe, token_url,
    };
    use crate::{Challenge, OciMiddleware};

    /// The challenges of a registry that answers `GET /v2/` with `header`.
    fn challenges(header: &str) -> Vec<Challenge> {
        let mut headers = http::HeaderMap::new();
        headers.insert(
            http::header::WWW_AUTHENTICATE,
            http::HeaderValue::from_str(header).unwrap(),
        );
        parse_challenges(&headers)
    }

    /// ghcr.io's challenge must keep producing exactly the token URL the
    /// middleware used to hardcode.
    #[test]
    fn bearer_challenge_builds_scoped_token_url() {
        let auth = registry_auth_from_challenges(&challenges(
            r#"Bearer realm="https://ghcr.io/token",service="ghcr.io""#,
        ));
        let RegistryAuth::TokenExchange { realm, service } = auth else {
            panic!("a Bearer challenge with a realm must yield a token exchange, got {auth:?}");
        };

        let url = token_url(
            &realm,
            service.as_deref(),
            "channel-mirrors/conda-forge/noarch/xtensor",
            OciAction::Pull,
        );

        assert_eq!(url.path(), "/token");
        // `scope` is percent-encoded here but not in the old hand-built URL;
        // registries decode the query either way.
        let query: Vec<_> = url.query_pairs().collect();
        assert_eq!(
            query,
            vec![
                ("service".into(), "ghcr.io".into()),
                (
                    "scope".into(),
                    "repository:channel-mirrors/conda-forge/noarch/xtensor:pull".into()
                ),
            ]
        );
    }

    /// A registry offering both schemes is negotiated with, even when `Basic`
    /// comes first, and a realm-only challenge is still usable.
    #[test]
    fn bearer_is_preferred_and_service_is_optional() {
        let auth = registry_auth_from_challenges(&challenges(
            r#"Basic realm="https://registry.example/", Bearer realm="https://registry.example/token""#,
        ));
        assert_eq!(
            auth,
            RegistryAuth::TokenExchange {
                realm: "https://registry.example/token".parse().unwrap(),
                service: None,
            }
        );
    }

    /// Amazon ECR only offers `Basic`: there is nothing to exchange, the stored
    /// credentials go straight onto the request.
    #[test]
    fn basic_challenge_sends_stored_credentials() {
        let auth = registry_auth_from_challenges(&challenges(
            r#"Basic realm="https://1234.dkr.ecr.eu-west-1.amazonaws.com/",service="ecr.amazonaws.com""#,
        ));
        assert_eq!(auth, RegistryAuth::Direct);

        let header = credentials_header(Some(&Authentication::BasicHTTP {
            username: "AWS".to_string(),
            password: "secret".to_string(),
        }))
        .expect("basic credentials are a valid header value");

        assert_eq!(header, "Basic QVdTOnNlY3JldA==");
        assert!(header.is_sensitive());
    }

    /// Without credentials we send no `Authorization` header at all and let the
    /// registry answer with its own error.
    #[test]
    fn no_credentials_means_no_authorization_header() {
        assert!(credentials_header(None).is_none());
        // Neither of these can travel in an `Authorization` header.
        assert!(credentials_header(Some(&Authentication::CondaToken("t".to_string()))).is_none());
        assert!(
            credentials_header(Some(&Authentication::S3Credentials {
                access_key_id: "k".to_string(),
                secret_access_key: "s".to_string(),
                session_token: None,
            }))
            .is_none()
        );
    }

    /// A challenge we cannot act on must degrade to the direct path instead of
    /// failing the download.
    #[test]
    fn unusable_challenges_degrade_to_direct() {
        for header in [
            r#"Bearer service="ghcr.io""#,
            r#"Bearer realm="not a url""#,
            r#"Bearer realm="http://registry.example/token""#,
            r#"Digest realm="https://registry.example/token""#,
            "%%% ###",
        ] {
            assert_eq!(
                registry_auth_from_challenges(&challenges(header)),
                RegistryAuth::Direct,
                "{header} should not be negotiated with"
            );
        }
        // A 2xx `GET /v2/` carries no challenge at all.
        assert_eq!(registry_auth_from_challenges(&[]), RegistryAuth::Direct);
    }

    #[test]
    fn transient_probe_responses_are_not_cached_as_direct() {
        assert_eq!(
            registry_auth_from_probe(StatusCode::TOO_MANY_REQUESTS, &[]),
            None
        );
        assert_eq!(
            registry_auth_from_probe(StatusCode::SERVICE_UNAVAILABLE, &[]),
            None
        );
        assert_eq!(
            registry_auth_from_probe(StatusCode::UNAUTHORIZED, &[]),
            None
        );
        assert_eq!(
            registry_auth_from_probe(StatusCode::OK, &[]),
            Some(RegistryAuth::Direct)
        );
        assert!(matches!(
            registry_auth_from_probe(
                StatusCode::UNAUTHORIZED,
                &challenges(r#"Bearer realm="https://registry.example/token""#)
            ),
            Some(RegistryAuth::TokenExchange { .. })
        ));
    }

    // test pulling an image from OCI registry
    #[cfg(any(feature = "rustls", feature = "native-tls"))]
    #[tokio::test]
    async fn test_oci_middleware() {
        let client = reqwest::Client::new();
        let middleware = OciMiddleware::new(client.clone());

        let client_with_middleware = reqwest_middleware::ClientBuilder::new(client)
            .with(middleware)
            .build();

        let response = client_with_middleware
            .get("oci://ghcr.io/channel-mirrors/conda-forge/osx-arm64/xtensor-0.25.0-h2ffa867_0.conda")
            .header(
                "X-Expected-Sha256",
                "8485a64911c7011c0270b8266ab2bffa1da41c59ac4f0a48000c31d4f4a966dd",
            )
            .send()
            .await
            .unwrap();

        // write out to tempfile
        assert_eq!(response.status(), 200);
        // check that the bytes are the same
        let hash = Sha256::digest(response.bytes().await.unwrap());
        assert_eq!(
            hex::encode(hash),
            "8485a64911c7011c0270b8266ab2bffa1da41c59ac4f0a48000c31d4f4a966dd"
        );
    }

    /// A digest the registry does not have falls back to the manifest, which
    /// still resolves the layer. This is the path that makes a pull-through
    /// cache import the artifact.
    #[cfg(any(feature = "rustls", feature = "native-tls"))]
    #[tokio::test]
    async fn test_oci_middleware_unknown_digest_falls_back_to_manifest() {
        let client = reqwest::Client::new();
        let middleware = OciMiddleware::new(client.clone());

        let client_with_middleware = reqwest_middleware::ClientBuilder::new(client)
            .with(middleware)
            .build();

        let response = client_with_middleware
            .get("oci://ghcr.io/channel-mirrors/conda-forge/osx-arm64/xtensor-0.25.0-h2ffa867_0.conda")
            .header(
                "X-Expected-Sha256",
                "0000000000000000000000000000000000000000000000000000000000000000",
            )
            .send()
            .await
            .unwrap();

        assert_eq!(response.status(), 200);
        let hash = Sha256::digest(response.bytes().await.unwrap());
        assert_eq!(
            hex::encode(hash),
            "8485a64911c7011c0270b8266ab2bffa1da41c59ac4f0a48000c31d4f4a966dd"
        );
    }

    /// Test that a missing package comes back as a plain 404.
    #[cfg(any(feature = "rustls", feature = "native-tls"))]
    #[tokio::test]
    async fn test_oci_middleware_missing_package_is_404() {
        let client = reqwest::Client::new();
        let middleware = OciMiddleware::new(client.clone());

        let client_with_middleware = reqwest_middleware::ClientBuilder::new(client)
            .with(middleware)
            .build();

        // Repo exists, version doesn't.
        let response = client_with_middleware
            .get("oci://ghcr.io/channel-mirrors/conda-forge/osx-arm64/xtensor-999.999.999-h0000000_0.conda")
            .send()
            .await
            .unwrap();

        assert_eq!(response.status(), 404);
    }

    /// The fallback must not turn a package that really is missing into an
    /// error: the manifest 404s too and that is the answer.
    #[cfg(any(feature = "rustls", feature = "native-tls"))]
    #[tokio::test]
    async fn test_oci_middleware_missing_package_with_digest_is_404() {
        let client = reqwest::Client::new();
        let middleware = OciMiddleware::new(client.clone());

        let client_with_middleware = reqwest_middleware::ClientBuilder::new(client)
            .with(middleware)
            .build();

        // Repo exists, version doesn't.
        let response = client_with_middleware
            .get("oci://ghcr.io/channel-mirrors/conda-forge/osx-arm64/xtensor-999.999.999-h0000000_0.conda")
            .header(
                "X-Expected-Sha256",
                "0000000000000000000000000000000000000000000000000000000000000000",
            )
            .send()
            .await
            .unwrap();

        assert_eq!(response.status(), 404);
    }

    // test pulling an image from OCI registry
    #[cfg(any(feature = "rustls", feature = "native-tls"))]
    #[tokio::test]
    async fn test_oci_middleware_repodata() {
        let client = reqwest::Client::new();
        let middleware = OciMiddleware::new(client.clone());

        let client_with_middleware = reqwest_middleware::ClientBuilder::new(client)
            .with(middleware)
            .build();

        let response = client_with_middleware
            .head("oci://ghcr.io/channel-mirrors/conda-forge/osx-arm64/repodata.json")
            .send()
            .await
            .unwrap();

        assert_eq!(response.status(), 200);
    }
}
