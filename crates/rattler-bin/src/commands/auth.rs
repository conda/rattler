use anyhow::Result;

#[cfg(feature = "cli-tools")]
use rattler::cli::auth;

#[cfg(feature = "cli-tools")]
pub type Opt = auth::Args;

#[cfg(not(feature = "cli-tools"))]
#[derive(Debug, clap::Parser)]
pub struct Opt {
    #[clap(subcommand)]
    command: AuthCommand,
}

#[cfg(not(feature = "cli-tools"))]
#[derive(Debug, clap::Subcommand)]
enum AuthCommand {
    Login(LoginOpt),
}

#[cfg(not(feature = "cli-tools"))]
#[derive(Debug, clap::Parser)]
struct LoginOpt {
    /// The host to authenticate with
    host: String,

    /// Authentication token
    #[clap(long)]
    token: String,
}

#[cfg(feature = "cli-tools")]
pub async fn auth(opt: Opt) -> Result<()> {
    auth::execute(opt).await.map_err(|e| anyhow::anyhow!(e))
}

#[cfg(not(feature = "cli-tools"))]
pub async fn auth(_opt: Opt) -> Result<()> {
    Err(anyhow::anyhow!(
        "Auth command requires cli-tools feature to be enabled"
    ))
}
