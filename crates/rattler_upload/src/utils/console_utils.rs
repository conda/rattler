use crate::utils::consts;

/// Checks whether we are on GitHub Actions
pub fn github_action_runner() -> bool {
    std::env::var(consts::GITHUB_ACTIONS) == Ok("true".to_string())
}

/// Checks whether we are on GitLab CI
pub fn gitlab_ci_runner() -> bool {
    std::env::var(consts::GITLAB_CI) == Ok("true".to_string())
}
