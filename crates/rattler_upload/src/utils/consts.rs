/// A `recipe.yaml` file might be accompanied by a `variants.yaml` file from
/// This env var is set to "true" when run inside a github actions runner
pub const GITHUB_ACTIONS: &str = "GITHUB_ACTIONS";
pub const GITHUB_REPOSITORY: &str = "GITHUB_REPOSITORY";
pub const GITHUB_WORKFLOW_REF: &str = "GITHUB_WORKFLOW_REF";
pub const GITHUB_WORKFLOW: &str = "GITHUB_WORKFLOW";
pub const GITHUB_REF: &str = "GITHUB_REF";
pub const GITHUB_ENVIRONMENT: &str = "GITHUB_ENVIRONMENT";

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
/// Environment variable to override the metadata server hostname (used by Google's libraries)
pub const GCE_METADATA_HOST: &str = "GCE_METADATA_HOST";
/// Default Google Cloud metadata server hostname
pub const GCP_METADATA_HOST_DEFAULT: &str = "metadata.google.internal";
/// Path to get identity tokens from the metadata server
pub const GCP_METADATA_IDENTITY_PATH: &str =
    "/computeMetadata/v1/instance/service-accounts/default/identity";
