use std::{env, path::PathBuf};

use miette::IntoDiagnostic;
use rattler_conda_types::{HasArtifactIdentificationRefs, PackageName, PrefixData};

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
    let header = [[
        "# Name".to_string(),
        "Version".to_string(),
        "Build".to_string(),
        "Channel".to_string(),
    ]];
    // These initial widths match the header columns length
    let mut widths: [usize; 4] = header[0].clone().map(|x| x.len());
    let mut lines = vec![];
    for record in prefix_data.iter() {
        if let Some(Ok(record)) = record {
            let name = record.name().as_normalized();
            if let Some(query) = &opt.name {
                let normalized_query = query.as_normalized();
                if opt.full_name {
                    if normalized_query != name {
                        continue;
                    }
                } else if !name.contains(normalized_query) {
                    continue;
                }
            };

            let fields = [
                name.to_string(),
                record.version().as_str().to_string(),
                record.build().to_string(),
                record.repodata_record.channel.clone().unwrap_or_default(),
            ];
            for (i, (field, width)) in fields.iter().zip(widths).enumerate() {
                let field_len = field.len();
                if field_len > width {
                    widths[i] = field_len;
                };
            }
            lines.push(fields);
        }
    }

    if let Some(query) = &opt.name {
        if lines.is_empty() {
            // If user queried a package but we didn't get matches, that's an error
            miette::bail!(
                "No packages matched {}query '{}'",
                if opt.full_name { "exact " } else { "" },
                query.as_normalized()
            );
        }
    }

    lines.sort();

    println!("# packages in environment at {}", prefix.to_string_lossy());
    for line in header.iter().chain(lines.iter()) {
        for (i, field) in line.iter().enumerate() {
            // Two spaces ----vv as inter-column padding
            print!("{:<width$}  ", field, width = widths[i]);
        }
        println!();
    }

    Ok(())
}
