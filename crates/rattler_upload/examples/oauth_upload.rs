//! Manual smoke test for the OAuth upload code path.
//!
//! Usage:
//!     cargo run -p rattler_upload --example oauth_upload -- \
//!         --host prefix.dev \
//!         --channel my-channel \
//!         path/to/package.conda
//!
//! This reads the credential at `--host` from the configured
//! `AuthenticationStorage` (keyring / `~/.rattler/auth.json` /
//! `$RATTLER_AUTH_FILE`), and uploads the given package via
//! `rattler_upload::upload::upload_package_to_prefix`. If the stored
//! credential is an `Authentication::OAuth { ... }`, the access token
//! is auto-refreshed when expired.
//!
//! Run `rattler auth login <host> --oauth` first to populate the
//! credential.

use std::path::PathBuf;

use rattler_networking::AuthenticationStorage;
use rattler_upload::upload::{
    opt::{AttestationSource, ForceOverwrite, PrefixData, SkipExisting},
    upload_package_to_prefix,
};
use url::Url;

#[derive(Debug)]
struct Args {
    host: String,
    channel: String,
    package: PathBuf,
}

fn parse_args() -> Args {
    let mut host = None;
    let mut channel = None;
    let mut package = None;

    let mut iter = std::env::args().skip(1);
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--host" => host = iter.next(),
            "--channel" => channel = iter.next(),
            other if package.is_none() => package = Some(PathBuf::from(other)),
            other => panic!("unexpected argument: {other}"),
        }
    }

    Args {
        host: host.expect("--host is required"),
        channel: channel.expect("--channel is required"),
        package: package.expect("package path is required"),
    }
}

#[tokio::main]
async fn main() {
    let args = parse_args();
    eprintln!(
        "uploading {:?} to {}/{}",
        args.package, args.host, args.channel
    );

    let storage = AuthenticationStorage::from_env_and_defaults()
        .expect("failed to construct AuthenticationStorage");

    let url: Url = format!("https://{}", args.host)
        .parse()
        .expect("invalid host");

    let prefix_data = PrefixData::new(
        url,
        args.channel,
        // None = force the upload code to look up the credential from
        // storage; this is the path that exercises OAuth.
        None,
        AttestationSource::NoAttestation,
        SkipExisting(false),
        ForceOverwrite(false),
        false,
    );

    match upload_package_to_prefix(&storage, &vec![args.package], prefix_data).await {
        Ok(()) => eprintln!("upload succeeded"),
        Err(e) => {
            eprintln!("upload failed: {e}");
            std::process::exit(1);
        }
    }
}
