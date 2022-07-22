//! The modules defines functionality to download channel [`crate::RepoData`] from several different
//! type of sources, cache the results, do this for several sources in parallel, and provide
//! adequate progress information to a user.

mod multi_request;
mod progress;
mod request;

pub use multi_request::{MultiRequestRepoDataBuilder, MultiRequestRepoDataListener};
pub use progress::terminal_progress;
pub use request::{
    DoneState, DownloadingState, RepoDataRequestState, RequestRepoDataBuilder,
    RequestRepoDataError, RequestRepoDataListener,
};
