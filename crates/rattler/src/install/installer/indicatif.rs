use crate::install::{Reporter, Transaction};
use indicatif::{HumanBytes, MultiProgress, ProgressFinish, ProgressState};
use parking_lot::Mutex;
use rattler_conda_types::{PrefixRecord, RepoDataRecord};
use std::borrow::Cow;
use std::fmt::Write;
use std::sync::Arc;
use std::time::Duration;

/// A builder to construct an [`IndicatifReporter`].
#[derive(Clone)]
pub struct IndicatifReporterBuilder {
    multi_progress: Option<indicatif::MultiProgress>,
    progress_chars: Cow<'static, str>,
    clear_when_done: bool,
    pending_style: Option<indicatif::ProgressStyle>,
    default_style: Option<indicatif::ProgressStyle>,
    default_pending_style: Option<indicatif::ProgressStyle>,
    download_style: Option<indicatif::ProgressStyle>,
    finish_style: Option<indicatif::ProgressStyle>,
}

impl IndicatifReporterBuilder {
    /// Sets the [`indicatif::MultiProgress`] to use for the progress bars.
    pub fn with_multi_progress(self, multi_progress: indicatif::MultiProgress) -> Self {
        Self {
            multi_progress: Some(multi_progress),
            ..self
        }
    }

    /// Sets whether the progress bars are cleared when the transaction is complete.
    pub fn clear_when_done(self, clear_when_done: bool) -> Self {
        Self {
            clear_when_done,
            ..self
        }
    }

    /// Finish building [`IndicatifReporter`].
    pub fn finish(self) -> IndicatifReporter {
        let multi_progress = self.multi_progress.unwrap_or_default();

        let pending_style = indicatif::ProgressStyle::with_template(
            "{spinner:.dim} {prefix:20!} [{elapsed_precise}] {msg:.dim}",
        )
        .expect("failed to create pending style");

        let default_style = indicatif::ProgressStyle::with_template("{spinner:.green} {prefix:20!} [{elapsed_precise}] [{bar:20!.bright.yellow/dim.white}] {pos:>4}/{len:4} {msg:.dim}")
            .expect("failed to create default style")
            .progress_chars(&self.progress_chars);

        let default_pending_style = indicatif::ProgressStyle::with_template("{spinner:.dim} {prefix:20!} [{elapsed_precise}] [{bar:20!.bright.yellow/dim.white}] {pos:>4}/{len:4} {msg:.dim}")
            .expect("failed to create default style")
            .progress_chars(&self.progress_chars);

        let download_style = indicatif::ProgressStyle::
            with_template("{spinner:.green} {prefix:20!} [{elapsed_precise}] [{bar:20!.bright.yellow/dim.white}] {bytes:>8} @ {smoothed_bytes_per_sec:8}").expect("failed to create download style")
            .progress_chars(&self.progress_chars)
            .with_key(
                "smoothed_bytes_per_sec",
                |s: &ProgressState, w: &mut dyn Write| match (s.pos(), s.elapsed().as_millis()) {
                    (pos, elapsed_ms) if elapsed_ms > 0 => {
                        write!(w, "{}/s", HumanBytes((pos as f64 * 1000_f64 / elapsed_ms as f64) as u64)).unwrap();
                    }
                    _ => write!(w, "-").unwrap(),
                },
            );

        let finish_style = indicatif::ProgressStyle::with_template(&format!(
            "{} {{prefix:20!}} [{{elapsed_precise}}] {{msg:.bold}}",
            console::style(console::Emoji("✔", " ")).green()
        ))
        .expect("failed to create finish style");

        IndicatifReporter {
            inner: Arc::new(Mutex::new(IndicatifReporterInner {
                multi_progress,
                pending_style: self.pending_style.unwrap_or(pending_style),
                default_pending_style: self
                    .default_pending_style
                    .clone()
                    .or_else(|| self.default_style.clone())
                    .unwrap_or(default_pending_style),
                default_style: self.default_style.unwrap_or(default_style),
                download_style: self.download_style.unwrap_or(download_style),
                finish_style: self.finish_style.unwrap_or(finish_style),
                validation_progress: None,
                download_progress: None,
                link_progress: None,
                total_packages_to_cache: 0,
                total_packages_cached: 0,
                total_download_size: 0,
                clear_when_done: self.clear_when_done,
                operations_in_progress: 0,
                bytes_downloaded: Vec::new(),
                package_sizes: Vec::new(),
            })),
        }
    }
}

/// A [`Reporter`] implementation to outputs progress bars using indicatif.
pub struct IndicatifReporter {
    inner: Arc<Mutex<IndicatifReporterInner>>,
}

struct IndicatifReporterInner {
    multi_progress: MultiProgress,

    pending_style: indicatif::ProgressStyle,
    default_style: indicatif::ProgressStyle,
    default_pending_style: indicatif::ProgressStyle,
    download_style: indicatif::ProgressStyle,
    finish_style: indicatif::ProgressStyle,

    validation_progress: Option<indicatif::ProgressBar>,
    download_progress: Option<indicatif::ProgressBar>,
    link_progress: Option<indicatif::ProgressBar>,

    clear_when_done: bool,
    operations_in_progress: usize,

    total_packages_to_cache: usize,
    total_packages_cached: usize,

    total_download_size: usize,
    bytes_downloaded: Vec<usize>,

    package_sizes: Vec<usize>,
}

impl IndicatifReporter {
    /// Returns a builder to construct a [`IndicatifReporter`].
    pub fn builder() -> IndicatifReporterBuilder {
        IndicatifReporterBuilder {
            multi_progress: None,
            progress_chars: Cow::Borrowed("━━╾─"),
            pending_style: None,
            default_style: None,
            default_pending_style: None,
            download_style: None,
            finish_style: None,
            clear_when_done: false,
        }
    }
}

impl Default for IndicatifReporter {
    fn default() -> Self {
        Self::builder().finish()
    }
}

impl Reporter for IndicatifReporter {
    fn on_transaction_start(&self, transaction: &Transaction<PrefixRecord, RepoDataRecord>) {
        let mut inner = self.inner.lock();

        let validation_progress = inner
            .multi_progress
            .add(indicatif::ProgressBar::new(0))
            .with_style(inner.pending_style.clone())
            .with_prefix("validate cache")
            .with_finish(ProgressFinish::AndLeave);
        validation_progress.enable_steady_tick(Duration::from_millis(100));

        let download_progress = inner
            .multi_progress
            .insert_after(&validation_progress, indicatif::ProgressBar::new(0))
            .with_style(inner.pending_style.clone())
            .with_prefix("download & extract")
            .with_finish(ProgressFinish::AndLeave);
        download_progress.enable_steady_tick(Duration::from_millis(100));

        let link_progress = inner
            .multi_progress
            .insert_after(&download_progress, indicatif::ProgressBar::new(0))
            .with_style(inner.pending_style.clone())
            .with_prefix("update prefix")
            .with_finish(ProgressFinish::AndLeave);
        link_progress.enable_steady_tick(Duration::from_millis(100));

        inner.total_packages_to_cache = transaction.packages_to_install();
        inner.total_packages_cached = 0;

        validation_progress.set_length(transaction.packages_to_install() as u64);
        download_progress.set_length(0);
        link_progress.set_length(
            (transaction.packages_to_install() + transaction.packages_to_uninstall()) as u64,
        );

        inner.validation_progress = Some(validation_progress);
        inner.download_progress = Some(download_progress);
        inner.link_progress = Some(link_progress);
    }

    fn on_transaction_operation_start(&self, _operation: usize) {}

    fn on_populate_cache_start(&self, operation: usize, record: &RepoDataRecord) -> usize {
        let mut inner = self.inner.lock();

        // Record the size of the record if we would download it.
        let new_length = inner.package_sizes.len().max(operation + 1);
        inner.package_sizes.resize_with(new_length, || 0);
        inner.package_sizes[operation] = record.package_record.size.unwrap_or_default() as usize;

        operation
    }

    fn on_validate_start(&self, _cache_entry: usize) -> usize {
        let inner = self.inner.lock();

        inner
            .validation_progress
            .as_ref()
            .expect("progress bar not set")
            .set_style(inner.default_style.clone());

        0
    }

    fn on_validate_complete(&self, _index: usize) {
        let inner = self.inner.lock();
        let validation_progress = inner
            .validation_progress
            .as_ref()
            .expect("progress bar not set");

        validation_progress.inc(1);
        if validation_progress.length() == Some(validation_progress.position()) {
            validation_progress.set_style(inner.finish_style.clone());
            validation_progress.finish_using_style();
        }
    }

    fn on_download_start(&self, cache_entry: usize) -> usize {
        let mut inner = self.inner.lock();

        let new_length = inner.package_sizes.len().max(cache_entry + 1);
        inner.bytes_downloaded.resize_with(new_length, || 0);
        inner.bytes_downloaded[cache_entry] = 0;
        inner.total_download_size += inner.package_sizes[cache_entry];

        let download_progress = inner
            .download_progress
            .as_ref()
            .expect("progress bar not set");
        download_progress.set_style(inner.download_style.clone());
        download_progress.set_length(inner.total_download_size as u64);
        cache_entry
    }

    fn on_download_progress(&self, cache_entry: usize, progress: u64, _total: Option<u64>) {
        let mut inner = self.inner.lock();

        inner.bytes_downloaded[cache_entry] = progress as usize;
        inner
            .download_progress
            .as_ref()
            .expect("progress bar not set")
            .set_position(inner.bytes_downloaded.iter().copied().sum::<usize>() as _);
    }

    fn on_download_completed(&self, _index: usize) {}

    fn on_populate_cache_complete(&self, _cache_entry: usize) {
        let mut inner = self.inner.lock();

        inner.total_packages_cached += 1;
        if inner.total_packages_cached >= inner.total_packages_to_cache {
            let validation_progress = inner
                .validation_progress
                .as_ref()
                .expect("progress bar not set");
            validation_progress.set_style(inner.finish_style.clone());
            validation_progress.finish_using_style();

            let download_progress = inner
                .download_progress
                .as_ref()
                .expect("progress bar not set");

            download_progress.set_style(inner.finish_style.clone());
            download_progress.finish_using_style();
        }
    }

    fn on_unlink_start(&self, _operation: usize, _record: &PrefixRecord) -> usize {
        let mut inner = self.inner.lock();

        inner.operations_in_progress += 1;

        if inner.operations_in_progress == 1 {
            inner
                .link_progress
                .as_ref()
                .expect("progress bar not set")
                .set_style(inner.default_style.clone());
        }

        0
    }

    fn on_unlink_complete(&self, _index: usize) {
        let mut inner = self.inner.lock();
        let link_progress = inner.link_progress.as_ref().expect("progress bar not set");
        link_progress.inc(1);

        inner.operations_in_progress -= 1;
        if inner.operations_in_progress == 0 {
            let link_progress = inner.link_progress.as_ref().expect("progress bar not set");
            link_progress.set_style(inner.default_pending_style.clone());
        }
    }

    fn on_link_start(&self, _operation: usize, _record: &RepoDataRecord) -> usize {
        let mut inner = self.inner.lock();

        inner.operations_in_progress += 1;
        if inner.operations_in_progress == 1 {
            inner
                .link_progress
                .as_ref()
                .expect("progress bar not set")
                .set_style(inner.default_style.clone());
        }

        0
    }

    fn on_link_complete(&self, _index: usize) {
        let mut inner = self.inner.lock();
        let link_progress = inner.link_progress.as_ref().expect("progress bar not set");
        link_progress.inc(1);

        inner.operations_in_progress -= 1;
        if inner.operations_in_progress == 0 {
            let link_progress = inner.link_progress.as_ref().expect("progress bar not set");
            link_progress.set_style(inner.default_pending_style.clone());
        }
    }

    fn on_transaction_operation_complete(&self, _operation: usize) {}

    fn on_transaction_complete(&self) {
        let inner = self.inner.lock();

        let validation_progress = inner
            .validation_progress
            .as_ref()
            .expect("progress bar not set");

        let download_progress = inner
            .download_progress
            .as_ref()
            .expect("progress bar not set");

        let link_progress = inner.link_progress.as_ref().expect("progress bar not set");

        validation_progress.set_style(inner.finish_style.clone());
        download_progress.set_style(inner.finish_style.clone());
        link_progress.set_style(inner.finish_style.clone());

        if inner.clear_when_done {
            validation_progress.finish_and_clear();
            download_progress.finish_and_clear();
            link_progress.finish_and_clear();
        } else {
            validation_progress.finish_using_style();
            download_progress.finish_using_style();
            link_progress.finish_using_style();
        }
    }
}
