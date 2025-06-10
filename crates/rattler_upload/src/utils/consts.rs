/// A `recipe.yaml` file might be accompanied by a `variants.yaml` file from
/// which we can read variant configuration for that specific recipe..
pub const VARIANTS_CONFIG_FILE: &str = "variants.yaml";

/// The name of the old-style configuration file (`conda_build_config.yaml`).
pub const CONDA_BUILD_CONFIG_FILE: &str = "conda_build_config.yaml";

/// This env var is set to "true" when run inside a github actions runner
pub const GITHUB_ACTIONS: &str = "GITHUB_ACTIONS";

/// This env var contains the oidc token url
pub const ACTIONS_ID_TOKEN_REQUEST_URL: &str = "ACTIONS_ID_TOKEN_REQUEST_URL";

/// This env var contains the oidc request token
pub const ACTIONS_ID_TOKEN_REQUEST_TOKEN: &str = "ACTIONS_ID_TOKEN_REQUEST_TOKEN";

// This env var determines whether GitHub integration is enabled
pub const RATTLER_BUILD_ENABLE_GITHUB_INTEGRATION: &str = "RATTLER_BUILD_ENABLE_GITHUB_INTEGRATION";
