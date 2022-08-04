//! Defines some useful [`MultiRequestRepoDataListener`]s.

use std::{collections::HashMap, time::Duration};

use indicatif::{MultiProgress, ProgressBar, ProgressDrawTarget, ProgressFinish, ProgressStyle};

use super::{DoneState, DownloadingState, MultiRequestRepoDataListener, RepoDataRequestState};
use crate::{Channel, Platform};

/// Returns a listener to use with the [`super::MultiRequestRepoDataBuilder`] that will show the
/// progress as several progress bars.
///
/// ```rust,no_run
/// # use rattler::{Channel, repo_data::fetch::{ terminal_progress, MultiRequestRepoDataBuilder}, ChannelConfig};
/// # tokio_test::block_on(async {
/// let _ = MultiRequestRepoDataBuilder::default()
///     .add_channel(Channel::from_str("conda-forge", &ChannelConfig::default()).unwrap())
///     .set_listener(terminal_progress())
///     .request()
///     .await;
/// # });
/// ```
pub fn terminal_progress() -> MultiRequestRepoDataListener {
    let multi_progress = MultiProgress::with_draw_target(ProgressDrawTarget::stderr_with_hz(10));
    let mut progress_bars = HashMap::<(Channel, Platform), ProgressBar>::new();

    // Construct a closure that captures the above variables. This closure will be called multiple
    // times during the lifetime of the request to notify of any state changes. The code below
    // will update a progressbar to reflect the state changes.
    Box::new(move |channel, platform, state| {
        // Find the progress bar that is associates with the given channel and platform. Or if no
        // such progress bar exists yet, create it.
        let progress_bar =
            progress_bars
                .entry((channel, platform))
                .or_insert_with_key(|(channel, platform)| {
                    let progress_bar = multi_progress.add(
                        ProgressBar::new(1)
                            .with_finish(ProgressFinish::AndLeave)
                            .with_prefix(format!(
                                "{}/{}",
                                channel
                                    .name
                                    .as_ref()
                                    .map(String::from)
                                    .unwrap_or_else(|| channel.canonical_name()),
                                platform
                            ))
                            .with_style(default_progress_style()),
                    );
                    progress_bar.enable_steady_tick(Duration::from_millis(100));
                    progress_bar
                });

        match state {
            RepoDataRequestState::Pending => {}
            RepoDataRequestState::Downloading(DownloadingState { bytes, total }) => {
                progress_bar.set_length(total.unwrap_or(bytes) as u64);
                progress_bar.set_position(bytes as u64);
            }
            RepoDataRequestState::Deserializing => {
                progress_bar.set_style(deserializing_progress_style());
                progress_bar.set_message("Deserializing..")
            }
            RepoDataRequestState::Done(DoneState {
                cache_miss: changed,
            }) => {
                progress_bar.set_style(finished_progress_style());
                if changed {
                    progress_bar.set_message("Done!");
                } else {
                    progress_bar.set_message("No changes!");
                }
                progress_bar.finish()
            }
            RepoDataRequestState::Error(_) => {
                progress_bar.set_style(errored_progress_style());
                progress_bar.finish_with_message("Error");
            }
        }
    })
}

/// Returns the style to use for a progressbar that is currently in progress.
fn default_progress_style() -> ProgressStyle {
    ProgressStyle::default_bar()
        .template("{spinner:.green} {prefix:20!} [{elapsed_precise}] [{bar:.bright.yellow/dim.white}] {bytes:>8} @ {bytes_per_sec:8}").unwrap()
        .progress_chars("━━╾─")
}

/// Returns the style to use for a progressbar that is in Deserializing state.
fn deserializing_progress_style() -> ProgressStyle {
    ProgressStyle::default_bar()
        .template("{spinner:.green} {prefix:20!} [{elapsed_precise}] [{bar:.bright.green/dim.white}] {wide_msg}").unwrap()
        .progress_chars("━━╾─")
}

/// Returns the style to use for a progressbar that is finished.
fn finished_progress_style() -> ProgressStyle {
    ProgressStyle::default_bar()
        .template("  {prefix:20!} [{elapsed_precise}] {msg:.bold}")
        .unwrap()
        .progress_chars("━━╾─")
}

/// Returns the style to use for a progressbar that is in error state.
fn errored_progress_style() -> ProgressStyle {
    ProgressStyle::default_bar()
        .template("  {prefix:20!} [{elapsed_precise}] {msg:.bold.red}")
        .unwrap()
        .progress_chars("━━╾─")
}
