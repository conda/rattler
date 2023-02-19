//! Contains models of files that are found in the `info/` directory of a package.

mod about;
mod files;
mod has_prefix;
mod index;
mod no_link;
mod no_softlink;
mod paths;
mod run_exports;

pub use {
    about::About,
    files::Files,
    has_prefix::HasPrefix,
    index::Index,
    no_link::NoLink,
    no_softlink::NoSoftlink,
    paths::{FileMode, PathType, PathsEntry, PathsJson},
    run_exports::RunExports,
};
