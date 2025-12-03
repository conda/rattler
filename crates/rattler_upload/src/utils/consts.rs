/// A `recipe.yaml` file might be accompanied by a `variants.yaml` file from
/// This env var is set to "true" when run inside a github actions runner
pub const GITHUB_ACTIONS: &str = "GITHUB_ACTIONS";

/// This env var contains the oidc token url
pub const ACTIONS_ID_TOKEN_REQUEST_URL: &str = "ACTIONS_ID_TOKEN_REQUEST_URL";

/// This env var contains the oidc request token
pub const ACTIONS_ID_TOKEN_REQUEST_TOKEN: &str = "ACTIONS_ID_TOKEN_REQUEST_TOKEN";

// GitLab CI environment variables
/// This env var is set to "true" when run inside a GitLab CI runner
pub const GITLAB_CI: &str = "GITLAB_CI";

/// The default env var name for the GitLab OIDC ID token with audience "prefix.dev".
/// Users should configure this in their `.gitlab-ci.yml` using the `id_tokens` keyword.
pub const PREFIX_ID_TOKEN: &str = "PREFIX_ID_TOKEN";

// Google Cloud environment variables
/// Set in Google Cloud Build
pub const CLOUD_BUILD_ID: &str = "CLOUD_BUILD_ID";
/// Set in Cloud Run
pub const K_SERVICE: &str = "K_SERVICE";
/// Google Cloud metadata server URL for identity tokens
pub const GCP_METADATA_IDENTITY_URL: &str =
    "http://metadata.google.internal/computeMetadata/v1/instance/service-accounts/default/identity";
