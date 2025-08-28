use rattler::cli::auth;

pub type Opt = auth::Args;

pub async fn auth(opt: Opt) -> miette::Result<()> {
    auth::execute(opt).await.map_err(|e| anyhow::anyhow!(e))
}
