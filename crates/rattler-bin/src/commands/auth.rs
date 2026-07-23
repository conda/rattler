use miette::IntoDiagnostic;
use rattler::cli::auth;

pub type Opt = auth::Args;

pub async fn auth(opt: Opt, offline: bool) -> miette::Result<()> {
    auth::execute_with_offline(opt, offline)
        .await
        .into_diagnostic()
}
