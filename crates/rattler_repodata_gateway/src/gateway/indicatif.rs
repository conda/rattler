use std::{borrow::Cow, collections::HashMap, sync::Arc, time::Duration};

use indicatif::{MultiProgress, ProgressBar, ProgressStyle};
use parking_lot::Mutex;
use url::Url;

use crate::{DownloadReporter, Reporter};

/// A builder to construct an [`IndicatifReporter`].
#[derive(Clone)]
pub struct IndicatifReporterBuilder {
    multi_progress: Option<MultiProgress>,
    clear_when_done: bool,
    prefix: Cow<'static, str>,
}

impl Default for IndicatifReporterBuilder {
    fn default() -> Self {
        Self::new()
    }
}

impl IndicatifReporterBuilder {
    /// Create a new builder.
    pub fn new() -> Self {
        Self {
            multi_progress: None,
            clear_when_done: false,
            prefix: Cow::Borrowed(""),
        }
    }

    /// Sets the [`MultiProgress`] to use for the progress bars.
    pub fn with_multi_progress(self, multi_progress: MultiProgress) -> Self {
        Self {
            multi_progress: Some(multi_progress),
            ..self
        }
    }

    /// Sets whether the progress bars are cleared when done.
    pub fn clear_when_done(self, clear_when_done: bool) -> Self {
        Self {
            clear_when_done,
            ..self
        }
    }

    /// Sets the prefix for all progress bars.
    pub fn with_prefix(self, prefix: impl Into<Cow<'static, str>>) -> Self {
        Self {
            prefix: prefix.into(),
            ..self
        }
    }

    /// Finish building [`IndicatifReporter`].
    pub fn finish(self) -> IndicatifReporter {
        let multi_progress = self.multi_progress.unwrap_or_default();

        IndicatifReporter {
            inner: Arc::new(Mutex::new(IndicatifReporterInner {
                multi_progress,
                downloads: HashMap::new(),
                clear_when_done: self.clear_when_done,
                prefix: self.prefix,
            })),
        }
    }
}

/// A [`Reporter`] implementation that outputs progress bars using indicatif.
#[derive(Clone)]
pub struct IndicatifReporter {
    inner: Arc<Mutex<IndicatifReporterInner>>,
}

struct IndicatifReporterInner {
    multi_progress: MultiProgress,
    downloads: HashMap<usize, ProgressBar>,
    clear_when_done: bool,
    prefix: Cow<'static, str>,
}

impl IndicatifReporter {
    /// Returns a builder to construct an [`IndicatifReporter`].
    pub fn builder() -> IndicatifReporterBuilder {
        IndicatifReporterBuilder::new()
    }
}

impl Default for IndicatifReporter {
    fn default() -> Self {
        Self::builder().finish()
    }
}

impl Reporter for IndicatifReporter {
    fn download_reporter(&self) -> Option<&dyn DownloadReporter> {
        Some(self)
    }
}

impl DownloadReporter for IndicatifReporter {
    fn on_download_start(&self, url: &Url) -> usize {
        let mut inner = self.inner.lock();
        let index = inner.downloads.len();

        let pb = inner.multi_progress.add(ProgressBar::new(0));

        pb.set_style(
            ProgressStyle::with_template(&format!(
                "{prefix}{{spinner:.green}} {{msg:.dim}}",
                prefix = if inner.prefix.is_empty() {
                    String::new()
                } else {
                    format!("{} ", inner.prefix)
                }
            ))
            .expect("failed to create progress style")
            .tick_chars("⠁⠂⠄⡀⢀⠠⠐⠈ "),
        );

        pb.set_message(format!("Fetching {}", simplify_url(url)));
        pb.enable_steady_tick(Duration::from_millis(100));

        inner.downloads.insert(index, pb);
        index
    }

    fn on_download_progress(
        &self,
        _url: &Url,
        index: usize,
        bytes_downloaded: usize,
        total_bytes: Option<usize>,
    ) {
        let inner = self.inner.lock();
        if let Some(pb) = inner.downloads.get(&index) {
            if let Some(total) = total_bytes {
                pb.set_length(total as u64);
                pb.set_position(bytes_downloaded as u64);
                pb.set_style(
                    ProgressStyle::with_template(&format!(
                        "{prefix}{{spinner:.green}} [{{bar:20.cyan/blue}}] {{bytes}}/{{total_bytes}} {{msg:.dim}}",
                        prefix = if inner.prefix.is_empty() {
                            String::new()
                        } else {
                            format!("{} ", inner.prefix)
                        }
                    ))
                    .expect("failed to create progress style")
                    .progress_chars("━━╾─")
                    .tick_chars("⠁⠂⠄⡀⢀⠠⠐⠈ "),
                );
            } else {
                pb.set_position(bytes_downloaded as u64);
            }
        }
    }

    fn on_download_complete(&self, _url: &Url, index: usize) {
        let mut inner = self.inner.lock();
        if let Some(pb) = inner.downloads.remove(&index) {
            if inner.clear_when_done {
                pb.finish_and_clear();
            } else {
                pb.finish_with_message("Done");
            }
        }
    }
}

fn simplify_url(url: &Url) -> String {
    if let Some(domain) = url.domain() {
        let path = url.path();
        if path.len() > 50 {
            let segments: Vec<&str> = path.split('/').collect();
            if let Some(last) = segments.last() {
                return format!("{domain}/.../{last}");
            }
        }
        format!("{domain}{path}")
    } else {
        url.to_string()
    }
}
