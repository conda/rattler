use std::path::PathBuf;

use miette::IntoDiagnostic;
use rattler_conda_types::{GenericVirtualPackage, Platform};
use rattler_virtual_packages::{VirtualPackageOverrides, VirtualPackages};
use serde::Serialize;

/// Show information about this `rattler` build and the host system.
#[derive(Debug, clap::Parser)]
#[clap(after_help = r#"Examples:
  rattler info
  rattler info --json"#)]
pub struct Opt {
    /// Output in JSON format
    #[clap(long)]
    json: bool,
}

/// The TLS backend `reqwest` was compiled against.
///
/// Both features can be enabled at once, in which case both are reported.
fn tls_backend() -> Vec<&'static str> {
    let mut backends = Vec::new();
    if cfg!(feature = "rustls") {
        backends.push("rustls");
    }
    if cfg!(feature = "native-tls") {
        backends.push("native-tls");
    }
    backends
}

/// The file storage backend that [`rattler_networking`] reads credentials from
/// by default. Mirrors `AuthenticationStorage::from_env_and_defaults`.
fn auth_storage_path() -> Option<PathBuf> {
    if let Ok(auth_file) = std::env::var("RATTLER_AUTH_FILE") {
        return Some(PathBuf::from(auth_file));
    }
    dirs::home_dir().map(|home| home.join(".rattler").join("credentials.json"))
}

/// The virtual packages detected for the current platform.
fn detect(cache_dir: Option<&std::path::Path>) -> miette::Result<Vec<String>> {
    let virtual_packages = VirtualPackages::detect(&VirtualPackageOverrides::from_env(), cache_dir)
        .into_diagnostic()?;

    Ok(virtual_packages
        .into_virtual_packages()
        .map(|package| GenericVirtualPackage::from(package).to_string())
        .collect())
}

#[derive(Debug, Serialize)]
struct Info {
    version: &'static str,
    platform: Platform,
    tls_backend: Vec<&'static str>,
    cache_dir: Option<PathBuf>,
    auth_storage: Option<PathBuf>,
    virtual_packages: Vec<String>,
}

const LABEL_WIDTH: usize = 18;

/// Print a single `label: value` line, right aligning the label so that all
/// values line up in a column.
fn print_field(label: &str, value: impl std::fmt::Display) {
    // Pad before styling; ANSI escapes count towards the format width.
    let label = format!("{label:>LABEL_WIDTH$}");
    println!("{}: {value}", console::style(label).bold());
}

/// Print `values` as a list, one per line. Only the first line carries the
/// label; the rest are indented to keep the values in one column.
fn print_list(label: &str, values: &[String]) {
    let Some((first, rest)) = values.split_first() else {
        print_field(label, console::style("none").dim());
        return;
    };

    print_field(label, first);
    for value in rest {
        // +2 for the ": " that `print_field` adds after the label.
        println!("{:width$}{value}", "", width = LABEL_WIDTH + 2);
    }
}

pub fn info(opt: Opt) -> miette::Result<()> {
    let cache_dir = rattler::default_cache_dir().ok();
    let virtual_packages = detect(cache_dir.as_deref())?;

    let info = Info {
        version: env!("CARGO_PKG_VERSION"),
        platform: Platform::current(),
        tls_backend: tls_backend(),
        cache_dir,
        auth_storage: auth_storage_path(),
        virtual_packages,
    };

    if opt.json {
        println!("{}", serde_json::to_string_pretty(&info).into_diagnostic()?);
        return Ok(());
    }

    print_field("Rattler version", info.version);
    print_field("Platform", info.platform);
    print_list(
        "TLS backend",
        &info
            .tls_backend
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>(),
    );
    print_field(
        "Cache dir",
        info.cache_dir.as_ref().map_or_else(
            || console::style("<disabled>".to_string()).dim().to_string(),
            |path| path.display().to_string(),
        ),
    );
    print_field(
        "Auth storage",
        info.auth_storage.as_ref().map_or_else(
            || console::style("<unknown>".to_string()).dim().to_string(),
            |path| path.display().to_string(),
        ),
    );
    print_list("Virtual packages", &info.virtual_packages);

    Ok(())
}
