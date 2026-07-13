use indicatif::{ProgressBar, ProgressStyle};
use std::{borrow::Cow, future::IntoFuture, time::Duration};

/// Displays a spinner with the given message while running the specified
/// function to completion.
pub fn wrap_in_progress<T, F: FnOnce() -> T>(msg: impl Into<Cow<'static, str>>, func: F) -> T {
    let pb = ProgressBar::new_spinner();
    pb.enable_steady_tick(Duration::from_millis(100));
    pb.set_style(long_running_progress_style());
    pb.set_message(msg);
    let result = func();
    pb.finish_and_clear();
    result
}

/// Displays a spinner with the given message while running the specified
/// function to completion.
pub async fn wrap_in_async_progress<T, F: IntoFuture<Output = T>>(
    msg: impl Into<Cow<'static, str>>,
    fut: F,
) -> T {
    let pb = ProgressBar::new_spinner();
    pb.enable_steady_tick(Duration::from_millis(100));
    pb.set_style(long_running_progress_style());
    pb.set_message(msg);
    let result = fut.into_future().await;
    pb.finish_and_clear();
    result
}

/// Returns the style to use for a progress bar that is indeterminate and simply
/// shows a spinner.
fn long_running_progress_style() -> indicatif::ProgressStyle {
    ProgressStyle::with_template("{spinner:.green} {msg}").unwrap()
}
