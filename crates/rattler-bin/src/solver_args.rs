//! Command line options shared by every command that resolves an environment.

use std::{collections::HashSet, fmt, str::FromStr, time::Duration};

use clap::ValueEnum;
use miette::IntoDiagnostic;
use rattler_conda_types::{
    Channel, ChannelConfig, GenericVirtualPackage, MatchSpec, Matches, ParseMatchSpecOptions,
    RepoDataRecord, SolverResult, Subdir, Version,
};
use rattler_config::{ConfigBase, NoExtension};
use rattler_repodata_gateway::{MultiSource, RepoData, Source};
use rattler_solve::{
    ChannelRepoData, IntoRepoData, SolveError, SolverImpl, SolverTask, libsolv_c, resolvo,
};
use rattler_virtual_packages::{VirtualPackageOverrides, VirtualPackages};

use crate::commands::gateway::resolve_channels;
use crate::exclude_newer::{ExcludeNewer, NamedCutoff};

/// Options that configure how an environment is solved.
///
/// Flatten this into a command's options with `#[clap(flatten)]` so that
/// every solving command accepts the same set of flags.
#[derive(Debug, clap::Args)]
pub struct SolverArgs {
    /// Channel to search for packages.
    ///
    /// A value of the form `NAME=CHANNEL,CHANNEL,...` searches the
    /// multichannel NAME instead, whose channels share a single channel
    /// priority tier.
    ///
    /// Example: `-c conda-forge -c defaults=https://repo.anaconda.com/pkgs/main,https://repo.anaconda.com/pkgs/r`
    #[clap(short, long = "channel")]
    channels: Vec<ChannelArg>,

    /// Additional constraint that the solution must satisfy.
    ///
    /// A constrained package is not necessarily part of the solution, but if
    /// it is, it must match the constraint.
    ///
    /// Example: --constraint "numpy<2" --constraint "openssl=3.*"
    #[clap(long = "constraint", value_name = "SPEC")]
    constraints: Vec<String>,

    /// The platform to solve for. Defaults to the platform of the current host.
    #[clap(long)]
    platform: Option<Subdir>,

    /// Virtual packages to use for solving, e.g. __glibc=2.28.
    ///
    /// When omitted, the virtual packages of the current system are detected.
    #[clap(long)]
    virtual_package: Option<Vec<String>>,

    /// SAT Solver backend to use.
    #[clap(long)]
    solver: Option<Solver>,

    /// Request solver timeout in milliseconds.
    #[clap(long)]
    timeout: Option<u64>,

    /// Solver strategy to use.
    #[clap(long)]
    strategy: Option<SolveStrategy>,

    /// How to prioritize packages from different channels.
    #[clap(long)]
    channel_priority: Option<ChannelPriority>,

    /// Only include dependencies of the package specs, not the specs themselves.
    #[clap(long, group = "deps_mode")]
    only_deps: bool,

    /// Only include the package specs themselves, without their dependencies.
    #[clap(long, group = "deps_mode")]
    no_deps: bool,

    /// Exclude packages newer than the specified cutoff.
    /// Can be specified as a timestamp (e.g., "2006-12-02T02:07:43Z"), a date
    /// (e.g., "2006-12-02"), or a duration (e.g., "3d").
    /// When using a date, packages from the entire day are included.
    #[clap(long)]
    exclude_newer: Option<ExcludeNewer>,

    /// Override the cutoff for a channel, as `CHANNEL=CUTOFF`.
    /// The cutoff accepts the same timestamp, date, and duration formats as
    /// `--exclude-newer`.
    /// May be specified multiple times.
    #[clap(
        long = "channel-cutoff",
        value_name = "CHANNEL=CUTOFF",
        requires = "exclude_newer"
    )]
    channel_cutoffs: Vec<NamedCutoff>,

    /// Allow records matching this spec regardless of `--exclude-newer`, for
    /// example `"polars ==1.43.1"`. The spec must name exactly one package.
    /// May be specified multiple times.
    #[clap(
        long = "exclude-newer-exemption",
        value_name = "SPEC",
        requires = "exclude_newer"
    )]
    exclude_newer_exemptions: Vec<String>,

    /// Policy for selecting package timestamps when using `--exclude-newer`.
    #[clap(long, default_value = "require-timestamp")]
    timestamp_policy: TimestampPolicy,
}

/// A `--channel` value: a single channel, or a multichannel given as
/// `NAME=CHANNEL,CHANNEL,...`.
#[derive(Debug, Clone, PartialEq, Eq)]
enum ChannelArg {
    /// A channel name, URL or path.
    Channel(String),
    /// A multichannel called `name` that consists of `channels`.
    MultiChannel { name: String, channels: Vec<String> },
}

#[derive(Debug)]
pub struct ParseMultiChannelArgError;

impl fmt::Display for ParseMultiChannelArgError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "expected NAME=CHANNEL,CHANNEL,... (for example, defaults=pkgs/main,pkgs/r)"
        )
    }
}

impl std::error::Error for ParseMultiChannelArgError {}

impl FromStr for ChannelArg {
    type Err = ParseMultiChannelArgError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        // Only a plain name before the `=` makes a multichannel: URLs and
        // Windows paths contain a `:` and other paths a separator, so a
        // channel that happens to contain a `=` stays a channel.
        let Some((name, channels)) = s
            .split_once('=')
            .filter(|(name, _)| !name.is_empty() && !name.contains(['/', '\\', ':']))
        else {
            return Ok(Self::Channel(s.to_string()));
        };
        let channels: Vec<String> = channels.split(',').map(str::to_string).collect();
        if channels.iter().any(String::is_empty) {
            return Err(ParseMultiChannelArgError);
        }
        Ok(Self::MultiChannel {
            name: name.to_string(),
            channels,
        })
    }
}

#[derive(Debug, Clone, Copy, ValueEnum)]
pub enum SolveStrategy {
    /// Resolve the highest compatible version for every package.
    Highest,

    /// Resolve the lowest compatible version for every package.
    Lowest,

    /// Resolve the lowest compatible version for direct dependencies but the
    /// highest compatible for transitive dependencies.
    LowestDirect,
}

impl From<SolveStrategy> for rattler_solve::SolveStrategy {
    fn from(value: SolveStrategy) -> Self {
        match value {
            SolveStrategy::Highest => rattler_solve::SolveStrategy::Highest,
            SolveStrategy::Lowest => rattler_solve::SolveStrategy::LowestVersion,
            SolveStrategy::LowestDirect => rattler_solve::SolveStrategy::LowestVersionDirect,
        }
    }
}

#[derive(Default, Debug, Clone, Copy, ValueEnum)]
pub enum TimestampPolicy {
    /// Prefer the indexed timestamp, then the build timestamp, and allow
    /// packages that have neither.
    AllowMissing,

    /// Prefer the indexed timestamp, then the build timestamp, and reject
    /// packages that have neither.
    #[default]
    RequireTimestamp,

    /// Use only the indexed timestamp and reject packages without one.
    RequireIndexedTimestamp,
}

impl From<TimestampPolicy> for rattler_solve::TimestampPolicy {
    fn from(value: TimestampPolicy) -> Self {
        match value {
            TimestampPolicy::AllowMissing => Self::AllowMissing,
            TimestampPolicy::RequireTimestamp => Self::RequireTimestamp,
            TimestampPolicy::RequireIndexedTimestamp => Self::RequireIndexedTimestamp,
        }
    }
}

#[derive(Default, Debug, Clone, Copy, ValueEnum)]
pub enum Solver {
    #[default]
    Resolvo,
    #[value(name = "libsolv")]
    LibSolv,
}

#[derive(Default, Debug, Clone, Copy, ValueEnum)]
pub enum ChannelPriority {
    /// A package is only taken from the first channel it is found in.
    #[default]
    Strict,

    /// Candidates from a higher-priority channel are exhausted before falling
    /// back to the next channel, regardless of version.
    Flexible,

    /// Packages can come from any channel, the version takes precedence.
    Disabled,
}

impl From<ChannelPriority> for rattler_solve::ChannelPriority {
    fn from(value: ChannelPriority) -> Self {
        match value {
            ChannelPriority::Strict => rattler_solve::ChannelPriority::Strict,
            ChannelPriority::Flexible => rattler_solve::ChannelPriority::Flexible,
            ChannelPriority::Disabled => rattler_solve::ChannelPriority::Disabled,
        }
    }
}

impl SolverArgs {
    /// Parses match specs as they are given on the command line.
    pub fn parse_specs(specs: &[String]) -> miette::Result<Vec<MatchSpec>> {
        let options = ParseMatchSpecOptions::strict()
            .with_extras(true)
            .with_conditionals(true)
            .with_flags(true);
        specs
            .iter()
            .map(|spec| MatchSpec::from_str(spec, options))
            .collect::<Result<Vec<_>, _>>()
            .into_diagnostic()
    }

    /// The constraints the solution must satisfy.
    pub fn constraints(&self) -> miette::Result<Vec<MatchSpec>> {
        Self::parse_specs(&self.constraints)
    }

    /// The channels to solve from, in the order they were given, or the
    /// configured channels if none were given, see [`resolve_channels`].
    pub fn channels(
        &self,
        config: &ConfigBase<NoExtension>,
        channel_config: &ChannelConfig,
    ) -> miette::Result<Vec<Source>> {
        if self.channels.is_empty() {
            return Ok(resolve_channels(None, config, channel_config)?
                .into_iter()
                .map(Source::from)
                .collect());
        }

        let mut names = HashSet::new();
        self.channels
            .iter()
            .map(|channel| match channel {
                ChannelArg::Channel(channel) => Channel::from_str(channel, channel_config)
                    .map(Source::from)
                    .into_diagnostic(),
                ChannelArg::MultiChannel { name, channels } => {
                    if !names.insert(name.as_str()) {
                        return Err(miette::miette!(
                            "multichannel '{name}' is given more than once"
                        ));
                    }
                    let channels = channels
                        .iter()
                        .map(|channel| Channel::from_str(channel, channel_config).map(Source::from))
                        .collect::<Result<_, _>>()
                        .into_diagnostic()?;
                    MultiSource::new(name.as_str(), channels)
                        .map(Source::from)
                        .into_diagnostic()
                }
            })
            .collect()
    }

    /// The platform to solve for, either as given on the command line or the
    /// platform of the current host.
    pub fn platform(&self) -> miette::Result<Subdir> {
        self.platform.map_or_else(crate::host_platform, Ok)
    }

    /// Whether explicit CLI capabilities replace all automatic detection.
    pub fn has_explicit_virtual_packages(&self) -> bool {
        self.virtual_package.is_some()
    }

    /// Builtin host capabilities, with environment overrides, for detector solves.
    pub fn builtin_virtual_packages(
        platform: Subdir,
    ) -> miette::Result<Vec<GenericVirtualPackage>> {
        VirtualPackages::detect_for_platform(
            platform,
            &VirtualPackageOverrides::from_env(),
            rattler::default_cache_dir().ok().as_deref(),
        )
        .map(|packages| packages.into_generic_virtual_packages().collect())
        .into_diagnostic()
    }

    /// The explicitly supplied capabilities, or the target's builtin capabilities.
    pub fn virtual_packages(&self) -> miette::Result<Vec<GenericVirtualPackage>> {
        let Some(virtual_packages) = &self.virtual_package else {
            return Self::builtin_virtual_packages(self.platform()?);
        };

        virtual_packages
            .iter()
            .map(|virt_pkg| {
                let elems = virt_pkg.split('=').collect::<Vec<&str>>();
                Ok(GenericVirtualPackage {
                    name: elems[0].try_into().into_diagnostic()?,
                    version: elems
                        .get(1)
                        .map_or(Version::from_str("0"), |s| Version::from_str(s))
                        .into_diagnostic()?,
                    build_string: (*elems.get(2).unwrap_or(&"")).to_string(),
                })
            })
            .collect()
    }

    pub fn timeout(&self) -> Option<Duration> {
        self.timeout.map(Duration::from_millis)
    }

    pub fn strategy(&self) -> rattler_solve::SolveStrategy {
        self.strategy.map_or_else(Default::default, Into::into)
    }

    pub fn channel_priority(&self) -> rattler_solve::ChannelPriority {
        self.channel_priority.unwrap_or_default().into()
    }

    pub fn exclude_newer(
        &self,
        channel_config: &ChannelConfig,
    ) -> miette::Result<Option<rattler_solve::ExcludeNewer>> {
        let Some(cutoff) = self.exclude_newer else {
            return Ok(None);
        };

        let now = jiff::Timestamp::now();
        let mut exclude_newer = cutoff.into_solver(now);
        let mut channels = HashSet::new();
        for override_ in &self.channel_cutoffs {
            let channel = Channel::from_str(&override_.name, channel_config).into_diagnostic()?;
            let channel = channel.canonical_name();
            if !channels.insert(channel.clone()) {
                return Err(miette::miette!(
                    "duplicate cutoff for channel '{}'",
                    override_.name
                ));
            }
            exclude_newer = override_
                .cutoff
                .apply_to_channel(exclude_newer, channel, now);
        }

        for spec in &self.exclude_newer_exemptions {
            let spec =
                MatchSpec::from_str(spec, ParseMatchSpecOptions::strict()).into_diagnostic()?;
            exclude_newer = exclude_newer.with_exemption(spec).into_diagnostic()?;
        }

        Ok(Some(
            exclude_newer.with_timestamp_policy(self.timestamp_policy.into()),
        ))
    }

    /// Solves the task with the selected backend.
    pub fn solve<'a, R, I>(&self, task: SolverTask<'a, I>) -> Result<SolverResult, SolveError>
    where
        I: IntoIterator<Item = R>,
        R: IntoRepoData<'a, resolvo::RepoData<'a>> + IntoRepoData<'a, libsolv_c::RepoData<'a>>,
    {
        match self.solver.unwrap_or_default() {
            Solver::Resolvo => resolvo::Solver.solve(task),
            Solver::LibSolv => libsolv_c::Solver.solve(task),
        }
    }

    /// Applies `--only-deps` / `--no-deps` to the solved records. A record is
    /// considered explicitly requested when it matches one of `specs`.
    pub fn filter_deps_mode(&self, records: &mut Vec<RepoDataRecord>, specs: &[MatchSpec]) {
        if self.no_deps {
            records.retain(|r| specs.iter().any(|s| s.matches(&r.package_record)));
        } else if self.only_deps {
            records.retain(|r| !specs.iter().any(|s| s.matches(&r.package_record)));
        }
    }
}

/// A solver task for `repo_data` that keeps track of the multichannel each
/// channel was requested through, so the channels of a multichannel share a
/// channel priority tier.
pub fn task_for_repodata(
    repo_data: &[RepoData],
) -> SolverTask<'_, Vec<ChannelRepoData<'_, &RepoData>>> {
    repo_data
        .iter()
        .map(|repo_data| ChannelRepoData {
            records: repo_data,
            multi_channel: repo_data.multi_channel(),
        })
        .collect()
}
