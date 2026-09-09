use std::path::PathBuf;

use miette::IntoDiagnostic;
use rattler_conda_types::{
    HasArtifactIdentificationRefs, PackageName, PrefixData, PrefixRecord, RepoDataRecord,
};

use crate::commands::{
    QueryOutputFormat, print_url_lines,
    table::{Cell, Table},
};

/// List the packages installed in a conda prefix.
#[derive(Debug, clap::Parser)]
#[clap(after_help = r#"Examples:
  rattler list -p /path/to/environment
  rattler list -p /path/to/environment --format json  # print the records as JSON
  rattler list -p /path/to/environment --format urls  # print only the package urls"#)]
pub struct Opt {
    /// Target prefix (environment path) to list
    #[clap(
        short = 'p',
        long = "prefix",
        visible_alias = "target-prefix",
        env = "CONDA_PREFIX"
    )]
    target_prefix: Option<PathBuf>,

    /// Only list packages whose name contains this string
    name: Option<PackageName>,

    /// Match full names only
    #[clap(short, long)]
    full_name: bool,

    /// Output format (defaults to human-readable output)
    #[clap(long)]
    format: Option<QueryOutputFormat>,
}

pub async fn list(opt: Opt) -> miette::Result<()> {
    let prefix = opt
        .target_prefix
        .ok_or_else(|| miette::miette!("No environment detected or passed. Tip: Use -p PATH."))?;
    let prefix = std::path::absolute(&prefix).into_diagnostic()?;

    let prefix_data = PrefixData::new(&prefix).into_diagnostic()?;
    let mut records: Vec<&PrefixRecord> = vec![];
    for record in prefix_data.iter() {
        let record = match record {
            Some(Ok(record)) => record,
            // A record that cannot be read makes the listing incomplete,
            // which should not go unnoticed, but it is no reason to withhold
            // the records that can be read.
            Some(Err(err)) => {
                tracing::warn!("skipping a conda-meta record that could not be read: {err}");
                continue;
            }
            None => continue,
        };

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

        records.push(record);
    }

    if let Some(query) = &opt.name
        && records.is_empty()
    {
        // If user queried a package but we didn't get matches, that's an error
        miette::bail!(
            "No packages matched {}query '{}'",
            if opt.full_name { "exact " } else { "" },
            query.as_normalized()
        );
    }

    records.sort_unstable_by(|a, b| {
        a.name()
            .cmp(b.name())
            .then_with(|| a.version().cmp(b.version()))
            .then_with(|| a.build().cmp(b.build()))
    });

    match opt.format {
        Some(QueryOutputFormat::Json) => {
            // Only the repodata part of each record is serialized; the file
            // lists of a prefix record are orders of magnitude larger than
            // the metadata and are available through `rattler inspect`.
            let repodata_records: Vec<&RepoDataRecord> = records
                .iter()
                .map(|record| &record.repodata_record)
                .collect();
            println!(
                "{}",
                serde_json::to_string_pretty(&repodata_records).into_diagnostic()?
            );
        }
        Some(QueryOutputFormat::Urls) => {
            print_url_lines(records.iter().map(|record| &record.repodata_record.url))?;
        }
        None => {
            let mut table = Table::with_header(["# Name", "Version", "Build", "Channel"]);
            for record in records {
                table.add_row(
                    [
                        record.name().as_normalized().to_string(),
                        record.version().as_str().to_string(),
                        record.build().to_string(),
                        record.repodata_record.channel.clone().unwrap_or_default(),
                    ]
                    .map(Cell::plain),
                );
            }

            println!("# packages in environment at {}", prefix.to_string_lossy());
            table.print();
        }
    }

    Ok(())
}
