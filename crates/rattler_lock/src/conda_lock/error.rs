//! Typed conversion diagnostics.

use std::{fmt, ops::Deref};

use rattler_conda_lock::{Diagnostic, ErrorKind, NodePath};

/// A conversion failure: a [`CondaLockErrorKind`] plus the CEP-37 model paths it
/// applies to, and the source spans of those paths when the conversion started
/// from a [`rattler_conda_lock::Document`].
///
/// Dereferences to the underlying [`Diagnostic`] for its labels, paths and
/// source text.
#[derive(Debug)]
pub struct CondaLockError(Box<Diagnostic<CondaLockErrorKind>>);

impl CondaLockError {
    /// Construct a conversion error at a CEP-37 model path.
    pub fn new(path: impl Into<NodePath>, kind: CondaLockErrorKind) -> Self {
        Self(Box::new(Diagnostic::new(path, kind)))
    }

    /// Add a related model path, for example the package a conflict came from.
    #[must_use]
    pub fn with_related_path(self, path: impl Into<NodePath>, message: &'static str) -> Self {
        Self(Box::new((*self.0).with_related_path(path, message)))
    }

    /// The underlying diagnostic, for example to render it with miette or to
    /// pass it to [`rattler_conda_lock::Document::contextualize`].
    pub fn into_diagnostic(self) -> Diagnostic<CondaLockErrorKind> {
        *self.0
    }
}

impl Deref for CondaLockError {
    type Target = Diagnostic<CondaLockErrorKind>;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl From<Diagnostic<CondaLockErrorKind>> for CondaLockError {
    fn from(diagnostic: Diagnostic<CondaLockErrorKind>) -> Self {
        Self(Box::new(diagnostic))
    }
}

impl From<rattler_conda_lock::Error> for CondaLockError {
    fn from(error: rattler_conda_lock::Error) -> Self {
        Self(Box::new(error.map_kind(CondaLockErrorKind::LockFile)))
    }
}

impl fmt::Display for CondaLockError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(&self.0, f)
    }
}

impl std::error::Error for CondaLockError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        self.0.kind().source()
    }
}

/// Which checksum a digest was expected to be.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ChecksumAlgorithm {
    /// The `md5` field.
    Md5,
    /// The `sha256` field.
    Sha256,
}

impl fmt::Display for ChecksumAlgorithm {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Md5 => "MD5",
            Self::Sha256 => "SHA256",
        })
    }
}

/// Why a lock file could not be converted.
///
/// Conversion drops metadata that does not change what gets installed, and
/// fails otherwise; every variant below names something that would have
/// silently changed an installed environment.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum CondaLockErrorKind {
    /// The CEP-37 lock file itself is invalid.
    #[error(transparent)]
    LockFile(ErrorKind),

    /// No destination environment was requested.
    #[error("at least one environment selection is required")]
    NoEnvironments,

    /// An environment name or its category set is empty.
    #[error("environment names and category selections must be nonempty")]
    EmptySelection,

    /// A selected category occurs in no package.
    #[error("category {category:?} does not occur in the lock file")]
    UnknownCategory {
        /// The requested category.
        category: String,
    },

    /// A target platform is not a platform pixi knows.
    #[error("unsupported target platform")]
    UnsupportedPlatform(#[source] rattler_conda_types::ParsePlatformError),

    /// The pixi lock-file builder rejected the imported data.
    #[error("cannot build a pixi lock file from this CEP-37 lock file")]
    Builder(#[source] crate::ParseCondaLockError),

    /// Two selected categories hold different packages under one name, so the
    /// destination environment cannot contain both.
    #[error("selected categories contain incompatible packages with the same name")]
    IncompatibleSelection,

    /// One artifact URL carries conflicting metadata in different categories.
    #[error("the same artifact has conflicting package metadata")]
    ConflictingArtifact,

    /// A package is built from source, which has no immutable artifact.
    #[error("source builds cannot be imported as immutable artifacts")]
    SourcePackage,

    /// A URL is not a direct artifact download.
    #[error("expected a direct artifact URL, not a source or local path")]
    NotAnArtifactUrl,

    /// A URL could not be parsed.
    #[error("invalid URL")]
    InvalidUrl(#[source] url::ParseError),

    /// A checksum is not valid hexadecimal of the required length. Importing
    /// validates the lock file first, which already rejects those, so this is
    /// the honest handling of a digest that cannot be decoded rather than a
    /// situation a valid document reaches.
    #[error("invalid {algorithm} digest")]
    InvalidDigest {
        /// The checksum that could not be decoded.
        algorithm: ChecksumAlgorithm,
    },

    /// Importing would have to rewrite the URL, which changes the artifact
    /// identity a pixi lock file records.
    #[error("conda artifact URL cannot be preserved without URL normalization")]
    UnnormalizedUrl,

    /// The URL has no conda archive file name.
    #[error("expected a conda archive filename")]
    MissingFileName,

    /// Neither the `build` field nor the URL yields a build string.
    #[error("build cannot be reconstructed from the artifact URL")]
    MissingBuildString,

    /// A conda package name is invalid.
    #[error("invalid conda package name")]
    InvalidCondaName(#[source] rattler_conda_types::InvalidPackageNameError),

    /// A conda version is invalid.
    #[error("invalid conda package version")]
    InvalidCondaVersion(#[source] rattler_conda_types::ParseVersionError),

    /// A Python package name is invalid.
    #[error("invalid Python package name")]
    InvalidPythonName(#[source] pep508_rs::InvalidNameError),

    /// A Python version is invalid.
    #[error("invalid Python package version")]
    InvalidPythonVersion(#[source] pep440_rs::VersionParseError),

    /// A dependency is not a valid PEP 508 requirement.
    #[error("invalid Python requirement")]
    InvalidRequirement(#[source] pep508_rs::Pep508Error),

    /// A dependency is not a valid conda `MatchSpec`.
    #[error("invalid conda dependency")]
    InvalidMatchSpec(#[source] rattler_conda_types::ParseMatchSpecError),

    /// The artifact subdir and the package's target platform disagree.
    #[error("artifact subdir disagrees with the selected platform")]
    SubdirMismatch,

    /// Two dependencies normalize to one name, so one would be lost.
    #[error("dependency names normalize to the same name")]
    AmbiguousDependencyName,

    /// A repeated dependency name cannot be a CEP-37 mapping key.
    #[error("repeated dependency name {name:?} cannot be represented in a CEP-37 mapping")]
    RepeatedDependencyName {
        /// The repeated dependency name.
        name: String,
    },

    /// A Python artifact carries a conda build string.
    #[error("Python artifacts cannot carry a conda build string")]
    PythonBuildString,

    /// A constraint lists alternatives, which PEP 508 cannot express.
    #[error("alternative version constraints cannot be represented in a Python requirement")]
    AlternativeConstraints,

    /// A dependency name is a matcher rather than an exact name.
    #[error("dependency names must be exact")]
    InexactDependencyName,

    /// A `MatchSpec` constrains more than CEP-37 dependency values can express.
    #[error("CEP-37 dependency values support only version and build constraints")]
    UnsupportedSpecFields,

    /// A `MatchSpec` cannot be written as a CEP-37 dependency value without
    /// changing which packages it matches.
    #[error("dependency cannot be represented without changing its meaning")]
    LossyDependency,

    /// A Python dependency carries extras or environment markers, which CEP-37
    /// dependency values cannot express.
    #[error("CEP-37 dependencies cannot preserve Python extras or environment markers")]
    PythonExtrasOrMarkers,

    /// A Python dependency points at a direct URL.
    #[error("CEP-37 dependencies cannot preserve direct Python dependency URLs")]
    DirectPythonUrl,

    /// Two pixi platforms map onto one CEP-37 subdir.
    #[error("multiple pixi platforms collapse to the same CEP-37 subdir")]
    CollapsingPlatforms,

    /// A pixi platform has a name that is not its subdir.
    #[error("custom platform names cannot be preserved in CEP-37")]
    CustomPlatformName,

    /// A platform requires virtual packages, which CEP-37 cannot record.
    #[error("custom virtual-package requirements cannot be represented in CEP-37")]
    VirtualPackages,

    /// One category cannot hold two artifacts with the same manager and name.
    #[error(
        "multiple artifacts with the same manager and name cannot be represented in one CEP-37 category"
    )]
    DuplicateExportedName,

    /// A conda package is built from source.
    #[error("conda source builds cannot be exported as CEP-37 artifacts")]
    CondaSourceBuild,

    /// A Python package is a local source tree.
    #[error("local Python source trees cannot be exported as CEP-37 artifacts")]
    PythonSourceTree,

    /// An artifact lives on the local file system rather than at a URL.
    #[error("local artifact paths cannot be exported as URLs")]
    LocalArtifactPath,

    /// The recorded file name cannot be derived from the URL, so a reader would
    /// reconstruct a different artifact.
    #[error("artifact filename cannot be reconstructed from its URL")]
    UnreconstructableFileName,

    /// The recorded subdir matches neither the URL nor the target platform.
    #[error("artifact subdir disagrees with both its URL and the selected platform")]
    SubdirDisagreement,

    /// The noarch install mode cannot be reconstructed from URL and build.
    #[error("noarch installation mode cannot be reconstructed from the artifact URL and build")]
    NoarchMismatch,

    /// A record installs into a custom site-packages directory.
    #[error("custom Python site-packages installation paths cannot be represented in CEP-37")]
    CustomSitePackagesPath,

    /// A record carries conditional dependencies.
    #[error("conditional or extra conda dependencies cannot be represented in CEP-37")]
    ExtraDepends,

    /// The verbatim URL and the installation URL disagree, so one of them would
    /// be lost.
    #[error("verbatim URL disagrees with the installation artifact URL")]
    VerbatimUrlMismatch,
}
