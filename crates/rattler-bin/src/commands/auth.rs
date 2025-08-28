use miette::IntoDiagnostic;
use rattler::cli::auth;

pub type Opt = auth::Args;

pub async fn auth(opt: Opt) -> miette::Result<()> {
    auth::execute(opt).await.into_diagnostic()
}
