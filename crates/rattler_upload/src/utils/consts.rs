/// A `recipe.yaml` file might be accompanied by a `variants.yaml` file from

/// This env var is set to "true" when run inside a github actions runner
pub const GITHUB_ACTIONS: &str = "GITHUB_ACTIONS";

/// This env var contains the oidc token url
pub const ACTIONS_ID_TOKEN_REQUEST_URL: &str = "ACTIONS_ID_TOKEN_REQUEST_URL";

/// This env var contains the oidc request token
pub const ACTIONS_ID_TOKEN_REQUEST_TOKEN: &str = "ACTIONS_ID_TOKEN_REQUEST_TOKEN";
