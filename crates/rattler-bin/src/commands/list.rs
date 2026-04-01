use std::io::Write;
use std::{env, path::PathBuf};

use itertools::Itertools;
use miette::IntoDiagnostic;
use rattler_conda_types::{HasArtifactIdentificationRefs, PackageName, PrefixData};
use tabwriter::TabWriter;

/// Search for packages in conda channels using glob or regex patterns.
#[derive(Debug, clap::Parser)]
#[clap(after_help = r#"Examples:
  rattler list -p /path/to/environment"#)]
pub struct Opt {
    /// The prefix to list
    #[clap(short, long)]
    prefix: Option<PathBuf>,

    /// The name (or glob) of the packages to list
    name: Option<PackageName>, // maybe this could be a full MatchSpec?

    /// Match full names only
    #[clap(short, long)]
    full_name: bool,
}

pub async fn list(opt: Opt) -> miette::Result<()> {
    let prefix = if let Some(prefix) = opt.prefix {
        prefix
    } else if let Ok(prefix) = env::var("CONDA_PREFIX") {
        PathBuf::from(prefix)
    } else {
        miette::bail!("No environment detected or passed. Tip: Use -p PATH.")
    };

    let prefix_data = PrefixData::new(&prefix).into_diagnostic()?;
    let mut tw = TabWriter::new(vec![]);
    let mut n_packages = 0usize;
    write!(&mut tw, "# Name\tVersion\tBuild\tChannel\n").unwrap();
    // These initial widths match the header columns length
    for name in prefix_data.package_names().sorted() {
        let record = prefix_data.get(name);
        if let Some(Ok(record)) = record {
            let namestr = name.as_normalized();
            if let Some(query) = &opt.name {
                let normalized_query = query.as_normalized();
                if opt.full_name {
                    if normalized_query != namestr {
                        continue;
                    }
                } else if !namestr.contains(normalized_query) {
                    continue;
                }
            };

            let fields = [
                namestr.to_string(),
                record.version().as_str().to_string(),
                record.build().to_string(),
                record.repodata_record.channel.clone().unwrap_or_default(),
            ];
            write!(&mut tw, "{}\n", fields.join("\t")).into_diagnostic()?;
            n_packages += 1;
        }
    }

    if let Some(query) = &opt.name {
        if n_packages == 0 {
            // If user queried a package but we didn't get matches, that's an error
            miette::bail!(
                "No packages matched {}query '{}'",
                if opt.full_name { "exact " } else { "" },
                query.as_normalized()
            );
        }
    }

    tw.flush().unwrap();
    println!(
        "# packages in environment at {}\n{}",
        prefix.to_string_lossy(),
        String::from_utf8(tw.into_inner().into_diagnostic()?).into_diagnostic()?
    );

    Ok(())
}
