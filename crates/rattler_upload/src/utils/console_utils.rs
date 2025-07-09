use crate::utils::consts;

/// Checks whether we are on GitHub Actions
pub fn github_action_runner() -> bool {
    std::env::var(consts::GITHUB_ACTIONS) == Ok("true".to_string())
}
