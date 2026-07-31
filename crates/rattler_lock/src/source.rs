//! Provides data types that are used to describe the location of a source
//! package.

use std::hash::{Hash, Hasher};

use rattler_digest::{Md5Hash, Sha256Hash};
use typed_path::Utf8TypedPathBuf;
use url::Url;

/// Describes the source location of a package.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum SourceLocation {
    /// The source is stored as a downloadable archive
    Url(UrlSourceLocation),

    /// The source is stored in a git repository
    Git(GitSourceLocation),

    /// The source is stored on the local filesystem
    Path(PathSourceLocation),
}

/// A specification of a source archive from a URL.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct UrlSourceLocation {
    /// The URL of the package contents
    pub url: Url,

    /// The md5 hash of the archive
    pub md5: Option<Md5Hash>,

    /// The sha256 hash of the archive
    pub sha256: Option<Sha256Hash>,

    /// The git subdirectory of the package
    pub subdirectory: Option<String>,
}

/// A specification of source from a git repository.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GitSourceLocation {
    /// The git url of the package which can contain git+ prefixes.
    pub git: Url,

    /// The git revision of the package
    pub rev: Option<GitReference>,

    /// The git subdirectory of the package
    pub subdirectory: Option<String>,

    /// Whether git LFS files should be fetched
    pub lfs: Option<bool>,
}

impl Hash for GitSourceLocation {
    fn hash<H: Hasher>(&self, state: &mut H) {
        let GitSourceLocation {
            git,
            rev,
            subdirectory,
            lfs,
        } = self;
        git.hash(state);
        rev.hash(state);
        subdirectory.hash(state);
        // An absent `lfs` field is skipped entirely so that source identifier
        // hashes of lock files written before the field existed stay stable.
        if let Some(lfs) = lfs {
            lfs.hash(state);
        }
    }
}

/// A reference to a specific commit in a git repository.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum GitReference {
    /// The HEAD commit of a branch.
    Branch(String),

    /// A specific tag.
    Tag(String),

    /// A specific commit.
    Rev(String),
}

/// Source located somewhere on the filesystem.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct PathSourceLocation {
    /// The path to the package. Either a directory or an archive.
    pub path: Utf8TypedPathBuf,
}
