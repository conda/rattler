//! Preparation and finalization around transaction package operations.

use std::{borrow::Borrow, collections::HashMap, path::PathBuf, sync::Arc};

use rattler_conda_types::{PackageRecord, PrefixRecord, prefix::Prefix};
use thiserror::Error;

use super::{
    Transaction, TransactionLinkContext,
    clobber_registry::{ClobberError, ClobberMode, ClobberedPath},
    installer::Reporter,
    link_script::{
        LinkScriptError, PrePostLinkResult, run_post_link_scripts, run_pre_unlink_scripts,
    },
    unlink::remove_empty_directories,
};

/// Policy for the preparation and finalization of a transaction.
#[derive(Debug, Clone, Copy, Default)]
pub struct TransactionOptions {
    /// Whether to execute pre-unlink and post-link scripts. Disabled by default.
    pub execute_link_scripts: bool,

    /// How to handle paths provided by multiple packages.
    pub clobber_mode: ClobberMode,
}

/// The script results and clobbered paths produced by a completed transaction.
#[derive(Debug)]
pub struct TransactionLifecycleResult {
    /// The pre-unlink script result, when scripts are enabled and reading their
    /// messages succeeded. Pre-unlink errors are logged during preparation.
    pub pre_link_script_result: Option<PrePostLinkResult>,

    /// The post-link script result, present when scripts are enabled.
    pub post_link_script_result: Option<Result<PrePostLinkResult, LinkScriptError>>,

    /// The paths provided by multiple packages during installation.
    pub clobbered_paths: HashMap<PathBuf, ClobberedPath>,
}

/// An error encountered while finalizing a transaction.
#[derive(Debug, Error)]
pub enum FinalizeTransactionError {
    /// Failed to restore the selected owners of clobbered files.
    #[error("failed to unclobber clobbered files")]
    ClobberError(#[from] ClobberError),

    /// Failed to determine the currently installed packages.
    #[error("failed to determine the installed packages")]
    FailedToDetectInstalledPackages(#[source] std::io::Error),

    /// Clobbering was detected with [`ClobberMode::Error`].
    #[error("{} file(s) are provided by multiple packages", .0.len())]
    ClobberingDetected(HashMap<PathBuf, ClobberedPath>),
}

/// A transaction prepared for package operations, bound to its prefix and reporter.
///
/// Run all package operations using the shared [`TransactionLinkContext`] before
/// consuming this value with [`Self::finalize`]. Preparation runs pre-unlink
/// scripts and removes menu entries; finalization resolves clobbers, cleans up
/// empty directories, and runs post-link scripts. Dropping this value does not
/// finalize or roll back the transaction.
///
/// ```no_run
/// use rattler::install::{
///     PreparedTransaction, Transaction, TransactionLifecycleResult,
///     TransactionLinkContext, TransactionOptions,
/// };
/// use rattler_conda_types::{PrefixRecord, RepoDataRecord, prefix::Prefix};
///
/// fn complete_transaction(
///     transaction: &Transaction<PrefixRecord, RepoDataRecord>,
///     prefix: &Prefix,
///     installed: &[PrefixRecord],
///     execute_operations: impl FnOnce(&TransactionLinkContext) -> Result<(), Box<dyn std::error::Error>>,
/// ) -> Result<TransactionLifecycleResult, Box<dyn std::error::Error>> {
///     let prepared = PreparedTransaction::prepare(
///         transaction,
///         prefix,
///         TransactionLinkContext::new().with_prefix_records(installed),
///         TransactionOptions::default(),
///         None,
///     );
///     let shared_context = prepared.link_context();
///     execute_operations(&shared_context)?;
///     Ok(prepared.finalize()?)
/// }
/// ```
pub struct PreparedTransaction<'a, Old, New> {
    transaction: &'a Transaction<Old, New>,
    prefix: &'a Prefix,
    reporter: Option<&'a dyn Reporter>,
    link_context: Arc<TransactionLinkContext>,
    options: TransactionOptions,
    pre_link_script_result: Option<PrePostLinkResult>,
}

impl<'a, Old: Borrow<PrefixRecord>, New> PreparedTransaction<'a, Old, New> {
    /// Run pre-unlink scripts and remove menu entries before package operations.
    ///
    /// Script errors and menu removal failures are logged, preserving the ability
    /// to remove packages even when their scripts or menu entries are broken.
    pub fn prepare(
        transaction: &'a Transaction<Old, New>,
        prefix: &'a Prefix,
        link_context: TransactionLinkContext,
        options: TransactionOptions,
        reporter: Option<&'a dyn Reporter>,
    ) -> Self {
        let mut pre_link_script_result = None;
        if options.execute_link_scripts {
            match run_pre_unlink_scripts(transaction, prefix, reporter) {
                Ok(result) => pre_link_script_result = Some(result),
                Err(error) => tracing::error!("Error running pre-unlink scripts: {:?}", error),
            }
        }

        for record in transaction.removed_packages() {
            let prefix_record: &PrefixRecord = record.borrow();
            if !prefix_record.installed_system_menus.is_empty() {
                match rattler_menuinst::remove_menu_items(&prefix_record.installed_system_menus) {
                    Ok(_) => {}
                    Err(error) => tracing::warn!("Failed to remove menu item: {}", error),
                }
            }
        }

        Self {
            transaction,
            prefix,
            reporter,
            link_context: Arc::new(link_context),
            options,
            pre_link_script_result,
        }
    }

    /// Share this transaction's linking state with package operation workers.
    pub fn link_context(&self) -> Arc<TransactionLinkContext> {
        Arc::clone(&self.link_context)
    }
}

impl<Old: Borrow<PrefixRecord> + AsRef<New>, New: AsRef<PackageRecord>>
    PreparedTransaction<'_, Old, New>
{
    /// Finalize after all package operations and prefix record writes complete.
    ///
    /// Resolves clobbers before checking [`ClobberMode::Error`]. A clobber error
    /// stops finalization before directory cleanup and post-link scripts.
    /// Cleanup failures are logged; post-link script errors are returned in the
    /// lifecycle result rather than failing finalization.
    pub fn finalize(self) -> Result<TransactionLifecycleResult, FinalizeTransactionError> {
        let prefix_records = PrefixRecord::collect_from_prefix(self.prefix)
            .map_err(FinalizeTransactionError::FailedToDetectInstalledPackages)?;
        let required_packages =
            PackageRecord::sort_topologically(prefix_records.iter().collect::<Vec<_>>());
        let clobbered_paths = self
            .link_context
            .clobber_registry()
            .unclobber(&required_packages, self.prefix)?;

        if self.options.clobber_mode == ClobberMode::Error && !clobbered_paths.is_empty() {
            return Err(FinalizeTransactionError::ClobberingDetected(
                clobbered_paths,
            ));
        }

        remove_empty_directories(&self.transaction.operations, &prefix_records, self.prefix)
            .unwrap_or_else(|error| {
                tracing::warn!("Failed to remove empty directories: {} (ignored)", error);
            });

        let post_link_script_result = if self.options.execute_link_scripts {
            Some(run_post_link_scripts(
                self.transaction,
                &required_packages,
                self.prefix,
                self.reporter,
            ))
        } else {
            None
        };

        Ok(TransactionLifecycleResult {
            pre_link_script_result: self.pre_link_script_result,
            post_link_script_result,
            clobbered_paths,
        })
    }
}
