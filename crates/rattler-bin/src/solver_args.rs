//! Command line options shared by every command that resolves an environment.

use std::{str::FromStr, time::Duration};

use clap::ValueEnum;
use miette::IntoDiagnostic;
use rattler_conda_types::{
    Channel, ChannelConfig, GenericVirtualPackage, MatchSpec, Matches, ParseMatchSpecOptions,
    Platform, RepoDataRecord, SolverResult, Version,
};
use rattler_solve::{IntoRepoData, SolveError, SolverImpl, SolverTask, libsolv_c, resolvo};
use rattler_virtual_packages::{VirtualPackageOverrides, VirtualPackages};

use crate::exclude_newer::ExcludeNewer;

/// Options that configure how an environment is solved.
///
/// Flatten this into a command's options with `#[clap(flatten)]` so that
/// every solving command accepts the same set of flags.
#[derive(Debug, clap::Args)]
pub struct SolverArgs {
    /// Channel to search for packages.
    ///
    /// Example: -c conda-forge -c main
    #[clap(short, long = "channel")]
    channels: Option<Vec<String>>,

    /// Additional constraint that the solution must satisfy.
    ///
    /// A constrained package is not necessarily part of the solution, but if
    /// it is, it must match the constraint.
    ///
    /// Example: --constraint "numpy<2" --constraint "openssl=3.*"
    #[clap(long = "constraint", value_name = "SPEC")]
    constraints: Vec<String>,

    /// The platform to solve for.
    #[clap(long, default_value_t = Platform::current())]
    pub platform: Platform,

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

    /// Exclude packages that have been published after the specified timestamp.
    /// Can be specified as a timestamp (e.g., "2006-12-02T02:07:43Z") or as a date (e.g., "2006-12-02").
    /// When using a date, packages from the entire day are included.
    #[clap(long)]
    exclude_newer: Option<ExcludeNewer>,
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

    /// The channels to solve from, defaulting to `conda-forge`.
    pub fn channels(&self, channel_config: &ChannelConfig) -> miette::Result<Vec<Channel>> {
        self.channels
            .clone()
            .unwrap_or_else(|| vec![String::from("conda-forge")])
            .into_iter()
            .map(|channel_str| Channel::from_str(channel_str, channel_config))
            .collect::<Result<Vec<_>, _>>()
            .into_diagnostic()
    }

    /// The virtual packages to solve with, either as given on the command line
    /// or detected from the current system.
    pub fn virtual_packages(&self) -> miette::Result<Vec<GenericVirtualPackage>> {
        let Some(virtual_packages) = &self.virtual_package else {
            return VirtualPackages::detect_for_platform(
                self.platform,
                &VirtualPackageOverrides::from_env(),
                rattler::default_cache_dir().ok().as_deref(),
            )
            .map(|vpkgs| vpkgs.into_generic_virtual_packages().collect::<Vec<_>>())
            .into_diagnostic();
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

    pub fn exclude_newer(&self) -> Option<rattler_solve::ExcludeNewer> {
        self.exclude_newer.map(Into::into)
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
