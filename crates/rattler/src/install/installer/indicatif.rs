use std::{
    borrow::Cow,
    cell::RefCell,
    collections::{HashMap, HashSet},
    sync::Arc,
    time::{Duration, Instant},
};

use indicatif::{HumanBytes, MultiProgress, ProgressFinish, ProgressStyle};
use parking_lot::Mutex;
use rattler_conda_types::{PrefixRecord, RepoDataRecord};

use crate::install::{Reporter, Transaction, TransactionOperation};

/// A builder to construct an [`IndicatifReporter`].
#[derive(Clone)]
pub struct IndicatifReporterBuilder<F: ProgressFormatter> {
    multi_progress: Option<indicatif::MultiProgress>,
    clear_when_done: bool,
    formatter: F,
    placement: Placement,
}

/// Defines how to place the progress bars. Note that the three progress bars
/// of the reporter are always kept together in the same order. This placement
/// refers to how the group of progress bars is placed.
#[derive(Debug, Clone, Default)]
pub enum Placement {
    /// Place all progress bars before the given progress bar.
    Before(indicatif::ProgressBar),

    /// Place all progress bars after the given progress bar
    After(indicatif::ProgressBar),

    /// Place all progress bars at the given index
    Index(usize),

    /// Place all progress bars as the last progress bars.
    #[default]
    End,
}

/// Defines the type of progress-bar.
#[derive(Debug, Copy, Clone, Hash, PartialEq, Eq)]
pub enum ProgressType {
    Generic,
    Bytes,
}

/// Defines the progress track.
#[derive(Debug, Copy, Clone, Hash, PartialEq, Eq)]
pub enum ProgressTrack {
    /// The progress of packages being validated in the cache.
    Validation,

    /// The progress of downloading and extracting packages to the cache.
    DownloadAndExtract,

    /// The progress of linking and unlinking extracted packages from the
    /// cache.
    Linking,
}

/// Defines the correct status of a progress bar.
#[derive(Debug, Copy, Clone, Hash, PartialEq, Eq)]
pub enum ProgressStatus {
    /// The progress bar is visible but has not started yet.
    Pending,

    /// The progress bar is showing active work.
    Active,

    /// The progress bar was active but has been paused for the moment.
    Paused,

    /// The progress bar finished.
    Finished,
}

/// Defines the properties of the progress bar.
#[derive(Debug, Clone, Hash, PartialEq, Eq)]
pub struct ProgressStyleProperties {
    /// True if there is work going on.
    pub status: ProgressStatus,

    /// The length and position of the progress bar are valid.
    pub determinate: bool,

    /// The type of progress to show.
    pub progress_type: ProgressType,

    /// The progress bar
    pub track: ProgressTrack,
}

/// A trait that can be used to customize the style of different progress bars
/// of a [`IndicatifReporter`].
pub trait ProgressFormatter {
    /// Returns a progress bar style for the given properties.
    fn format(&self, props: &ProgressStyleProperties) -> indicatif::ProgressStyle;
}

/// A default implementation of a [`ProgressFormatter`].
pub struct DefaultProgressFormatter {
    progress_chars: Cow<'static, str>,
    prefix: Cow<'static, str>,
}

impl Default for DefaultProgressFormatter {
    fn default() -> Self {
        Self {
            progress_chars: Cow::Borrowed("━━╾─"),
            prefix: Cow::Borrowed(""),
        }
    }
}

impl ProgressFormatter for DefaultProgressFormatter {
    fn format(&self, props: &ProgressStyleProperties) -> indicatif::ProgressStyle {
        let mut result = self.prefix.to_string();

        // Add a spinner
        match props.status {
            ProgressStatus::Pending | ProgressStatus::Paused => result.push_str("{spinner:.dim} "),
            ProgressStatus::Active => result.push_str("{spinner:.green} "),
            ProgressStatus::Finished => result.push_str(&format!(
                "{} ",
                console::style(console::Emoji("✔", " ")).green()
            )),
        }

        // Add a prefix
        result.push_str("{prefix:20!} ");

        // Add progress indicator
        if props.determinate && props.status != ProgressStatus::Finished {
            if props.status == ProgressStatus::Active {
                result.push_str("[{elapsed_precise}] [{bar:20!.bright.yellow/dim.white}] ");
            } else {
                result.push_str("[{elapsed_precise}] [{bar:20!.dim.yellow/dim.white}] ");
            }
            match props.progress_type {
                ProgressType::Generic if props.track == ProgressTrack::Linking => {
                    result.push_str("{human_pos:>5}/{human_len} ");
                }
                ProgressType::Generic => {
                    // Don't show position and total, because these are visible
                    // through text anyway.
                }
                ProgressType::Bytes => result.push_str("{bytes:>8} @ {bytes_per_sec:8} "),
            }
        } else {
            result.push_str("");
        }

        // Add message
        result.push_str("{msg:.dim}");

        indicatif::ProgressStyle::with_template(&result)
            .expect("failed to create default style")
            .progress_chars(&self.progress_chars)
    }
}

impl DefaultProgressFormatter {
    /// Sets the prefix all all progress bars.
    pub fn with_prefix(self, prefix: impl Into<Cow<'static, str>>) -> Self {
        Self {
            prefix: prefix.into(),
            ..self
        }
    }
}

impl<F: ProgressFormatter> IndicatifReporterBuilder<F> {
    /// Sets the formatter to use for the progress bars.
    pub fn with_formatter<T: ProgressFormatter>(self, formatter: T) -> IndicatifReporterBuilder<T> {
        IndicatifReporterBuilder {
            multi_progress: self.multi_progress,
            clear_when_done: self.clear_when_done,
            placement: self.placement,
            formatter,
        }
    }

    /// Sets the [`indicatif::MultiProgress`] to use for the progress bars.
    pub fn with_multi_progress(self, multi_progress: indicatif::MultiProgress) -> Self {
        Self {
            multi_progress: Some(multi_progress),
            ..self
        }
    }

    /// Sets whether the progress bars are cleared when the transaction is
    /// complete.
    pub fn clear_when_done(self, clear_when_done: bool) -> Self {
        Self {
            clear_when_done,
            ..self
        }
    }

    /// Defines how the progress bars of the reporter are placed relative to
    /// any other progress bars that are already present.
    pub fn with_placement(self, placement: Placement) -> Self {
        Self { placement, ..self }
    }

    /// Finish building [`IndicatifReporter`].
    pub fn finish(self) -> IndicatifReporter<F> {
        let multi_progress = self.multi_progress.unwrap_or_default();

        IndicatifReporter {
            inner: Arc::new(Mutex::new(IndicatifReporterInner {
                multi_progress,
                formatter: self.formatter,
                style_cache: RefCell::default(),
                validation_progress: None,
                download_progress: None,
                link_progress: None,
                total_packages_to_cache: 0,
                total_packages_cached: 0,
                packages_validating: HashSet::default(),
                packages_validated: HashSet::default(),
                packages_downloading: HashSet::default(),
                packages_downloaded: HashSet::default(),
                total_download_size: 0,
                clear_when_done: self.clear_when_done,
                operations_in_progress: HashSet::default(),
                bytes_downloaded: Vec::new(),
                package_sizes: Vec::new(),
                package_names: Vec::new(),
                start_validating: None,
                start_downloading: None,
                start_linking: None,
                end_validating: None,
                end_downloading: None,
                end_linking: None,
                placement: self.placement,
                populate_cache_started: HashSet::default(),
            })),
        }
    }
}

/// A [`Reporter`] implementation to outputs progress bars using indicatif.
pub struct IndicatifReporter<F> {
    inner: Arc<Mutex<IndicatifReporterInner<F>>>,
}

struct IndicatifReporterInner<F> {
    multi_progress: MultiProgress,

    formatter: F,
    style_cache: RefCell<HashMap<ProgressStyleProperties, ProgressStyle>>,

    validation_progress: Option<indicatif::ProgressBar>,
    download_progress: Option<indicatif::ProgressBar>,
    link_progress: Option<indicatif::ProgressBar>,

    populate_cache_started: HashSet<usize>,

    clear_when_done: bool,

    total_packages_to_cache: usize,
    total_packages_cached: usize,

    packages_validating: HashSet<usize>,
    packages_validated: HashSet<usize>,

    packages_downloading: HashSet<usize>,
    packages_downloaded: HashSet<usize>,

    total_download_size: usize,
    bytes_downloaded: Vec<usize>,

    package_sizes: Vec<usize>,
    package_names: Vec<String>,

    operations_in_progress: HashSet<usize>,

    start_validating: Option<Instant>,
    start_downloading: Option<Instant>,
    start_linking: Option<Instant>,

    end_validating: Option<Instant>,
    end_downloading: Option<Instant>,
    end_linking: Option<Instant>,
    placement: Placement,
}

impl<F: ProgressFormatter> IndicatifReporterInner<F> {
    fn style(&self, props: ProgressStyleProperties) -> ProgressStyle {
        self.style_cache
            .borrow_mut()
            .entry(props.clone())
            .or_insert_with(|| self.formatter.format(&props))
            .clone()
    }

    fn update_validating_message(&self) {
        let Some(validation_progress) = &self.validation_progress else {
            return;
        };

        validation_progress.set_message(self.format_progress_message(&self.packages_validating));
    }

    fn update_validating_status(&self) {
        let Some(validation_progress) = &self.validation_progress else {
            return;
        };

        if self.packages_validating.is_empty() {
            if self.populate_cache_started.len() == self.total_packages_to_cache {
                self.finish_validation_progress();
            } else {
                validation_progress.set_style(self.style(ProgressStyleProperties {
                    status: ProgressStatus::Paused,
                    determinate: true,
                    progress_type: ProgressType::Generic,
                    track: ProgressTrack::Validation,
                }));
            }
        }
    }

    fn finish_validation_progress(&self) {
        let Some(validation_progress) = &self.validation_progress else {
            return;
        };

        validation_progress.set_style(self.style(ProgressStyleProperties {
            status: ProgressStatus::Finished,
            determinate: true,
            progress_type: ProgressType::Generic,
            track: ProgressTrack::Validation,
        }));
        validation_progress.finish_using_style();
        if let (Some(start), Some(end)) = (self.start_validating, self.end_validating) {
            validation_progress.set_message(format!(
                "{} {} in {}",
                self.packages_validated.len(),
                if self.packages_validated.len() == 1 {
                    "package"
                } else {
                    "packages"
                },
                format_duration(end - start)
            ));
        }
    }

    fn update_download_message(&self) {
        let Some(download_progress) = &self.download_progress else {
            return;
        };

        download_progress.set_message(self.format_progress_message(&self.packages_downloading));
    }

    fn update_linking_message(&self) {
        let Some(link_progress) = &self.link_progress else {
            return;
        };

        link_progress.set_message(self.format_progress_message(&self.operations_in_progress));
    }

    fn format_progress_message(&self, remaining: &HashSet<usize>) -> String {
        let mut msg = String::new();

        // Sort the packages from large to small.
        let package_iter = remaining
            .iter()
            .map(|&idx| (self.package_sizes[idx], &self.package_names[idx]));

        let largest_package = package_iter.max_by_key(|(size, _)| *size);
        if let Some((_, first)) = largest_package {
            msg.push_str(first);
        }

        let count = remaining.len();
        if count > 1 {
            msg.push_str(&format!(" (+{})", count - 1));
        }

        msg
    }
}

impl IndicatifReporter<DefaultProgressFormatter> {
    /// Returns a builder to construct a [`IndicatifReporter`].
    pub fn builder() -> IndicatifReporterBuilder<DefaultProgressFormatter> {
        IndicatifReporterBuilder {
            multi_progress: None,
            clear_when_done: false,
            formatter: DefaultProgressFormatter::default(),
            placement: Placement::default(),
        }
    }
}

impl Default for IndicatifReporter<DefaultProgressFormatter> {
    fn default() -> Self {
        Self::builder().finish()
    }
}

impl<F: ProgressFormatter + Send> Reporter for IndicatifReporter<F> {
    fn on_transaction_start(&self, transaction: &Transaction<PrefixRecord, RepoDataRecord>) {
        let mut inner = self.inner.lock();

        let link_progress = match &inner.placement {
            Placement::Before(pb) => inner
                .multi_progress
                .insert_before(pb, indicatif::ProgressBar::new(0)),
            Placement::After(pb) => inner
                .multi_progress
                .insert_after(pb, indicatif::ProgressBar::new(0)),
            Placement::Index(idx) => inner
                .multi_progress
                .insert(*idx, indicatif::ProgressBar::new(0)),
            Placement::End => inner.multi_progress.add(indicatif::ProgressBar::new(0)),
        };

        let link_progress = link_progress
            .with_style(inner.style(ProgressStyleProperties {
                status: ProgressStatus::Pending,
                determinate: true,
                progress_type: ProgressType::Generic,
                track: ProgressTrack::Linking,
            }))
            .with_prefix("installing packages")
            .with_finish(ProgressFinish::AndLeave);
        link_progress.enable_steady_tick(Duration::from_millis(100));

        link_progress.set_length(
            (transaction.packages_to_install() + transaction.packages_to_uninstall()) as u64,
        );

        inner.link_progress = Some(link_progress);
        inner.total_packages_to_cache = transaction.packages_to_install();

        inner.package_names.reserve(transaction.operations.len());
        inner.package_sizes.reserve(transaction.operations.len());
        for operation in &transaction.operations {
            let record = match operation {
                TransactionOperation::Install(new) | TransactionOperation::Change { new, .. } => {
                    &new.package_record
                }
                TransactionOperation::Reinstall(old) | TransactionOperation::Remove(old) => {
                    &old.repodata_record.package_record
                }
            };
            inner
                .package_names
                .push(record.name.as_normalized().to_string());
            inner
                .package_sizes
                .push(record.size.unwrap_or_default() as usize);
        }
    }

    fn on_transaction_operation_start(&self, _operation: usize) {}

    fn on_populate_cache_start(&self, operation: usize, _record: &RepoDataRecord) -> usize {
        let mut inner = self.inner.lock();

        inner.populate_cache_started.insert(operation);

        operation
    }

    fn on_validate_start(&self, cache_entry: usize) -> usize {
        let mut inner = self.inner.lock();

        inner.packages_validating.insert(cache_entry);

        inner.start_validating.get_or_insert_with(Instant::now);

        let validation_progress = match &inner.validation_progress {
            Some(pb) => pb,
            None => {
                let place_above = inner
                    .download_progress
                    .as_ref()
                    .or_else(|| inner.link_progress.as_ref())
                    .expect("progress bar not set");

                let pb = inner
                    .multi_progress
                    .insert_before(place_above, indicatif::ProgressBar::new(0))
                    .with_style(inner.style(ProgressStyleProperties {
                        status: ProgressStatus::Active,
                        determinate: true,
                        progress_type: ProgressType::Generic,
                        track: ProgressTrack::Validation,
                    }))
                    .with_prefix("validate cache")
                    .with_finish(ProgressFinish::AndLeave);
                pb.enable_steady_tick(Duration::from_millis(100));

                inner.validation_progress = Some(pb);
                inner
                    .validation_progress
                    .as_ref()
                    .expect("progress bar not set")
            }
        };

        validation_progress.inc_length(1);
        validation_progress.set_style(inner.style(ProgressStyleProperties {
            status: ProgressStatus::Active,
            determinate: true,
            progress_type: ProgressType::Generic,
            track: ProgressTrack::Validation,
        }));

        inner.update_validating_message();

        cache_entry
    }

    fn on_validate_complete(&self, cache_entry: usize) {
        let mut inner = self.inner.lock();

        inner.packages_validating.remove(&cache_entry);
        inner.packages_validated.insert(cache_entry);

        inner.end_validating = Some(Instant::now());

        let validation_progress = inner
            .validation_progress
            .as_ref()
            .expect("progress bar not set");

        validation_progress.inc(1);

        inner.update_validating_message();
        inner.update_validating_status();
    }

    fn on_download_start(&self, cache_entry: usize) -> usize {
        let mut inner = self.inner.lock();

        inner.packages_downloading.insert(cache_entry);

        inner.start_downloading.get_or_insert_with(Instant::now);

        let new_length = inner.package_sizes.len().max(cache_entry + 1);
        inner.bytes_downloaded.resize_with(new_length, || 0);
        inner.bytes_downloaded[cache_entry] = 0;
        inner.total_download_size += inner.package_sizes[cache_entry];

        let download_progress = match &inner.download_progress {
            Some(pb) => pb,
            None => {
                let place_above = inner.link_progress.as_ref().expect("progress bar not set");

                let pb = inner
                    .multi_progress
                    .insert_before(place_above, indicatif::ProgressBar::new(0))
                    .with_style(inner.style(ProgressStyleProperties {
                        status: ProgressStatus::Active,
                        determinate: true,
                        progress_type: ProgressType::Generic,
                        track: ProgressTrack::DownloadAndExtract,
                    }))
                    .with_prefix("download & extract")
                    .with_finish(ProgressFinish::AndLeave);
                pb.enable_steady_tick(Duration::from_millis(100));

                inner.download_progress = Some(pb);
                inner
                    .download_progress
                    .as_ref()
                    .expect("progress bar not set")
            }
        };

        download_progress.set_style(inner.style(ProgressStyleProperties {
            status: ProgressStatus::Active,
            determinate: true,
            progress_type: ProgressType::Bytes,
            track: ProgressTrack::DownloadAndExtract,
        }));
        download_progress.set_length(inner.total_download_size as u64);

        inner.update_download_message();
        inner.update_validating_message();
        inner.update_validating_status();

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

    fn on_download_completed(&self, cache_entry: usize) {
        let mut inner = self.inner.lock();
        inner.end_downloading = Some(Instant::now());

        inner.packages_downloading.remove(&cache_entry);
        inner.packages_downloaded.insert(cache_entry);

        if inner.packages_downloading.is_empty() {
            inner
                .download_progress
                .as_ref()
                .expect("progress bar not set")
                .set_style(inner.style(ProgressStyleProperties {
                    status: ProgressStatus::Paused,
                    determinate: true,
                    progress_type: ProgressType::Bytes,
                    track: ProgressTrack::DownloadAndExtract,
                }));
        }

        inner.update_download_message();
    }

    fn on_populate_cache_complete(&self, _cache_entry: usize) {
        let mut inner = self.inner.lock();

        inner.total_packages_cached += 1;
        if inner.total_packages_cached >= inner.total_packages_to_cache {
            inner.finish_validation_progress();

            if let Some(download_pb) = &inner.download_progress {
                download_pb.set_style(inner.style(ProgressStyleProperties {
                    status: ProgressStatus::Finished,
                    determinate: true,
                    progress_type: ProgressType::Bytes,
                    track: ProgressTrack::DownloadAndExtract,
                }));
                download_pb.finish_using_style();
                if let (Some(start), Some(end)) = (inner.start_downloading, inner.end_downloading) {
                    download_pb.set_message(format!(
                        "{} {} ({}) in {}",
                        inner.packages_downloaded.len(),
                        if inner.packages_downloaded.len() == 1 {
                            "package"
                        } else {
                            "packages"
                        },
                        HumanBytes(inner.bytes_downloaded.iter().sum::<usize>() as u64),
                        format_duration(end - start)
                    ));
                }
            }
        }
    }

    fn on_unlink_start(&self, operation: usize, _record: &PrefixRecord) -> usize {
        let mut inner = self.inner.lock();

        inner.start_linking.get_or_insert_with(Instant::now);
        inner.operations_in_progress.insert(operation);

        if inner.operations_in_progress.len() == 1 {
            inner
                .link_progress
                .as_ref()
                .expect("progress bar not set")
                .set_style(inner.style(ProgressStyleProperties {
                    status: ProgressStatus::Active,
                    determinate: true,
                    progress_type: ProgressType::Generic,
                    track: ProgressTrack::Linking,
                }));
        }

        inner.update_linking_message();

        operation
    }

    fn on_unlink_complete(&self, operation: usize) {
        let mut inner = self.inner.lock();
        let link_progress = inner.link_progress.as_ref().expect("progress bar not set");
        link_progress.inc(1);

        inner.end_linking = Some(Instant::now());

        inner.operations_in_progress.remove(&operation);
        if inner.operations_in_progress.is_empty() {
            let link_progress = inner.link_progress.as_ref().expect("progress bar not set");
            link_progress.set_style(inner.style(ProgressStyleProperties {
                status: ProgressStatus::Paused,
                determinate: true,
                progress_type: ProgressType::Generic,
                track: ProgressTrack::Linking,
            }));
        }

        inner.update_linking_message();
    }

    fn on_link_start(&self, operation: usize, _record: &RepoDataRecord) -> usize {
        let mut inner = self.inner.lock();

        inner.start_linking.get_or_insert_with(Instant::now);

        inner.operations_in_progress.insert(operation);
        if inner.operations_in_progress.len() == 1 {
            inner
                .link_progress
                .as_ref()
                .expect("progress bar not set")
                .set_style(inner.style(ProgressStyleProperties {
                    status: ProgressStatus::Active,
                    determinate: true,
                    progress_type: ProgressType::Generic,
                    track: ProgressTrack::Linking,
                }));
        }

        inner.update_linking_message();

        operation
    }

    fn on_link_complete(&self, operation: usize) {
        let mut inner = self.inner.lock();
        let link_progress = inner.link_progress.as_ref().expect("progress bar not set");
        link_progress.inc(1);

        inner.end_linking = Some(Instant::now());

        inner.operations_in_progress.remove(&operation);
        if inner.operations_in_progress.is_empty() {
            let link_progress = inner.link_progress.as_ref().expect("progress bar not set");
            link_progress.set_style(inner.style(ProgressStyleProperties {
                status: ProgressStatus::Paused,
                determinate: true,
                progress_type: ProgressType::Generic,
                track: ProgressTrack::Linking,
            }));
        }

        inner.update_linking_message();
    }

    fn on_transaction_operation_complete(&self, _operation: usize) {}

    fn on_transaction_complete(&self) {
        let mut inner = self.inner.lock();

        if let (Some(link_pb), Some(start), Some(end)) =
            (&inner.link_progress, inner.start_linking, inner.end_linking)
        {
            link_pb.set_message(format!("took {}", format_duration(end - start)));
        }

        for (pb, track) in [
            (inner.validation_progress.take(), ProgressTrack::Validation),
            (
                inner.download_progress.take(),
                ProgressTrack::DownloadAndExtract,
            ),
            (inner.link_progress.take(), ProgressTrack::Linking),
        ] {
            let Some(pb) = pb else { continue };
            pb.set_style(inner.style(ProgressStyleProperties {
                status: ProgressStatus::Finished,
                determinate: true,
                progress_type: if track == ProgressTrack::DownloadAndExtract {
                    ProgressType::Bytes
                } else {
                    ProgressType::Generic
                },
                track,
            }));
            if inner.clear_when_done {
                pb.finish_and_clear();
            } else {
                pb.finish_using_style();
            }
        }
    }
}

/// Formats a durations. Rounds to milliseconds and uses human-readable format.
fn format_duration(duration: Duration) -> humantime::FormattedDuration {
    humantime::format_duration(Duration::from_millis(duration.as_millis() as u64))
}
