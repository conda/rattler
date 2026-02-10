use crate::utils::consts;

/// Checks whether we are on GitHub Actions
pub fn github_action_runner() -> bool {
    std::env::var(consts::GITHUB_ACTIONS) == Ok("true".to_string())
}

/// Checks whether we are on GitLab CI
pub fn gitlab_ci_runner() -> bool {
    std::env::var(consts::GITLAB_CI) == Ok("true".to_string())
}

/// Checks whether we are on Google Cloud (Cloud Build, Cloud Run, GCE, etc.)
pub fn google_cloud_runner() -> bool {
    std::env::var(consts::CLOUD_BUILD_ID).is_ok() || std::env::var(consts::K_SERVICE).is_ok()
}
