//! Provides an solver implementation based on the [`rattler_libsolv_c`] crate.

use std::{
    collections::{HashMap, HashSet},
    ffi::CString,
    mem::ManuallyDrop,
};

pub use input::cache_repodata;
use input::{add_repodata_records, add_solv_file, add_virtual_packages};
pub use libc_byte_slice::LibcByteSlice;
use output::get_required_packages;
use rattler_conda_types::{MatchSpec, NamelessMatchSpec, RepoDataRecord};
use wrapper::{
    flags::SolverFlag,
    pool::{Pool, Verbosity},
    repo::Repo,
    solve_goal::SolveGoal,
};

use crate::{ChannelPriority, IntoRepoData, SolveError, SolveStrategy, SolverRepoData, SolverTask};

mod input;
mod libc_byte_slice;
mod output;
mod wrapper;

/// Represents the information required to load available packages into libsolv
/// for a single channel and platform combination
#[derive(Clone)]
pub struct RepoData<'a> {
    /// The actual records after parsing `repodata.json`
    pub records: Vec<&'a RepoDataRecord>,

    /// The in-memory .solv file built from the records (if available)
    pub solv_file: Option<&'a LibcByteSlice>,
}

impl<'a> FromIterator<&'a RepoDataRecord> for RepoData<'a> {
    fn from_iter<T: IntoIterator<Item = &'a RepoDataRecord>>(iter: T) -> Self {
        Self {
            records: Vec::from_iter(iter),
            solv_file: None,
        }
    }
}

impl<'a> RepoData<'a> {
    /// Constructs a new `LibsolvRsRepoData`
    #[deprecated(since = "0.6.0", note = "use From::from instead")]
    pub fn from_records(records: impl Into<Vec<&'a RepoDataRecord>>) -> Self {
        Self {
            records: records.into(),
            solv_file: None,
        }
    }
}

impl<'a> SolverRepoData<'a> for RepoData<'a> {}

/// Convenience method that converts a string reference to a `CString`,
/// replacing NUL characters with whitespace (`b' '`)
fn c_string<T: AsRef<str>>(str: T) -> CString {
    let bytes = str.as_ref().as_bytes();

    let mut vec = Vec::with_capacity(bytes.len() + 1);
    vec.extend_from_slice(bytes);

    for byte in &mut vec {
        if *byte == 0 {
            *byte = b' ';
        }
    }

    // Trailing 0
    vec.push(0);

    // Safe because the string does is guaranteed to have no NUL bytes other than
    // the trailing one
    unsafe { CString::from_vec_with_nul_unchecked(vec) }
}

/// A [`Solver`] implemented using the `libsolv` library
#[derive(Default)]
pub struct Solver;

impl super::SolverImpl for Solver {
    type RepoData<'a> = RepoData<'a>;

    fn solve<
        'a,
        R: IntoRepoData<'a, Self::RepoData<'a>>,
        TAvailablePackagesIterator: IntoIterator<Item = R>,
    >(
        &mut self,
        task: SolverTask<TAvailablePackagesIterator>,
    ) -> Result<Vec<RepoDataRecord>, SolveError> {
        if task.timeout.is_some() {
            return Err(SolveError::UnsupportedOperations(vec![
                "timeout".to_string()
            ]));
        }

        if task.strategy != SolveStrategy::Highest {
            return Err(SolveError::UnsupportedOperations(vec![
                "strategy".to_string()
            ]));
        }

        // Construct a default libsolv pool
        let pool = Pool::default();

        // Setup proper logging for the pool
        pool.set_debug_callback(|msg, _flags| {
            tracing::event!(tracing::Level::DEBUG, "{}", msg.trim_end());
        });
        pool.set_debug_level(Verbosity::Low);

        let repodatas: Vec<Self::RepoData<'_>> = task
            .available_packages
            .into_iter()
            .map(IntoRepoData::into)
            .collect();

        // Determine the channel priority for each channel in the repodata in the order
        // in which the repodatas are passed, where the first channel will have
        // the highest priority value and each successive channel will descend
        // in priority value. If not strict, the highest priority value will be
        // 0 and the channel priority map will not be populated as it will
        // not be used.
        let mut highest_priority: i32 = 0;
        let channel_priority: HashMap<String, i32> =
            if task.channel_priority == ChannelPriority::Strict {
                let mut seen_channels = HashSet::new();
                let mut channel_order: Vec<String> = Vec::new();
                for channel in repodatas
                    .iter()
                    .filter(|&r| !r.records.is_empty())
                    .map(|r| r.records[0].channel.clone())
                {
                    if !seen_channels.contains(&channel) {
                        channel_order.push(channel.clone());
                        seen_channels.insert(channel);
                    }
                }
                let mut channel_priority = HashMap::new();
                for (index, channel) in channel_order.iter().enumerate() {
                    let reverse_index = channel_order.len() - index;
                    if index == 0 {
                        highest_priority = reverse_index as i32;
                    }
                    channel_priority.insert(channel.clone(), reverse_index as i32);
                }
                channel_priority
            } else {
                HashMap::new()
            };

        // Add virtual packages
        let repo = Repo::new(&pool, "virtual_packages", highest_priority);
        add_virtual_packages(&pool, &repo, &task.virtual_packages);

        // Mark the virtual packages as installed.
        pool.set_installed(&repo);

        // Create repos for all channel + platform combinations
        let mut repo_mapping = HashMap::new();
        let mut all_repodata_records = Vec::new();
        for repodata in repodatas.iter() {
            if repodata.records.is_empty() {
                continue;
            }
            let channel_name = &repodata.records[0].channel;

            // We dont want to drop the Repo, its stored in the pool anyway.
            let priority: i32 = if task.channel_priority == ChannelPriority::Strict {
                *channel_priority.get(channel_name).unwrap()
            } else {
                0
            };
            let repo = ManuallyDrop::new(Repo::new(&pool, channel_name, priority));

            if let Some(solv_file) = repodata.solv_file {
                add_solv_file(&pool, &repo, solv_file);
            } else {
                add_repodata_records(
                    &pool,
                    &repo,
                    repodata.records.iter().copied(),
                    task.exclude_newer.as_ref(),
                )?;
            }

            // Keep our own info about repodata_records
            repo_mapping.insert(repo.id(), repo_mapping.len());
            all_repodata_records.push(repodata.records.clone());
        }

        // Create a special pool for records that are already installed or locked.
        let repo = Repo::new(&pool, "locked", highest_priority);
        let installed_solvables = add_repodata_records(&pool, &repo, &task.locked_packages, None)?;

        // Also add the installed records to the repodata
        repo_mapping.insert(repo.id(), repo_mapping.len());
        all_repodata_records.push(task.locked_packages.iter().collect());

        // Create a special pool for records that are pinned and cannot be changed.
        let repo = Repo::new(&pool, "pinned", highest_priority);
        let pinned_solvables = add_repodata_records(&pool, &repo, &task.pinned_packages, None)?;

        // Also add the installed records to the repodata
        repo_mapping.insert(repo.id(), repo_mapping.len());
        all_repodata_records.push(task.pinned_packages.iter().collect());

        // Create datastructures for solving
        pool.create_whatprovides();

        // Add matchspec to the queue
        let mut goal = SolveGoal::default();

        // Favor the currently installed packages
        for favor_solvable in installed_solvables {
            goal.favor(favor_solvable);
        }

        // Lock the currently pinned packages
        for locked_solvable in pinned_solvables {
            goal.lock(locked_solvable);
        }

        // Specify the matchspec requests
        for spec in task.specs {
            let id = pool.intern_matchspec(&spec);
            goal.install(id, false);
        }

        for spec in task.constraints {
            let id = pool.intern_matchspec(&spec);
            goal.install(id, true);
        }

        // Add virtual packages to the queue. We want to install these as part of the
        // solution as well. This ensures that if a package only has a constraint on a
        // virtual package, the virtual package is installed.
        for virtual_package in task.virtual_packages {
            let id = pool.intern_matchspec(&MatchSpec::from_nameless(
                NamelessMatchSpec::default(),
                Some(virtual_package.name),
            ));
            goal.install(id, false);
        }

        // Construct a solver and solve the problems in the queue
        let mut solver = pool.create_solver();
        solver.set_flag(SolverFlag::allow_uninstall(), true);
        solver.set_flag(SolverFlag::allow_downgrade(), true);
        solver.set_flag(
            SolverFlag::strict_channel_priority(),
            task.channel_priority == ChannelPriority::Strict,
        );

        let transaction = solver.solve(&mut goal).map_err(SolveError::Unsolvable)?;

        let required_records = get_required_packages(
            &pool,
            &repo_mapping,
            &transaction,
            all_repodata_records.as_slice(),
        )
        .map_err(|unsupported_operation_ids| {
            SolveError::UnsupportedOperations(
                unsupported_operation_ids
                    .into_iter()
                    .map(|id| format!("libsolv operation {id}"))
                    .collect(),
            )
        })?;

        Ok(required_records)
    }
}

#[cfg(test)]
mod test {
    use rstest::rstest;

    use super::*;

    #[rstest]
    #[case("", "")]
    #[case("a\0b\0c\0d\0", "a b c d ")]
    #[case("a b c d", "a b c d")]
    #[case("😒", "😒")]
    fn test_c_string(#[case] input: &str, #[case] expected_output: &str) {
        let output = c_string(input);
        assert_eq!(output.as_bytes(), expected_output.as_bytes());
    }
}
