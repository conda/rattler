//! Typed diagnostics: a static [`ErrorKind`], the model path it occurred at,
//! and—once a [`crate::Document`] has been consulted—the source span.

use std::{fmt, ops::Range, path::PathBuf, sync::Arc};

use crate::Manager;

/// A path to a node in the lock-file model, such as `package[3].hash.sha256`.
///
/// Paths are built from [`NodePath::root`] with [`NodePath::field`] and
/// [`NodePath::index`] so that diagnostics and the source map agree on one
/// spelling. Field names are appended verbatim, including names that contain a
/// `.`; [`crate::Document`] resolves those by exact lookup before shortening.
#[derive(Clone, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct NodePath(String);

impl NodePath {
    /// The path of the document itself.
    pub fn root() -> Self {
        Self(String::new())
    }

    /// The path of a child field, or of a mapping entry with key `name`.
    #[must_use]
    pub fn field(&self, name: &str) -> Self {
        let mut path = String::with_capacity(self.0.len() + name.len() + 1);
        path.push_str(&self.0);
        if !self.0.is_empty() {
            path.push('.');
        }
        path.push_str(name);
        Self(path)
    }

    /// The path of the `index`th element of this sequence.
    #[must_use]
    pub fn index(&self, index: usize) -> Self {
        Self(format!("{}[{index}]", self.0))
    }

    /// The dotted spelling of this path. Empty for the document itself.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Whether this path denotes the whole document.
    pub fn is_root(&self) -> bool {
        self.0.is_empty()
    }
}

impl fmt::Display for NodePath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl fmt::Debug for NodePath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(&self.0, f)
    }
}

impl From<&str> for NodePath {
    fn from(value: &str) -> Self {
        Self(value.to_owned())
    }
}

impl From<String> for NodePath {
    fn from(value: String) -> Self {
        Self(value)
    }
}

impl AsRef<str> for NodePath {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

impl PartialEq<str> for NodePath {
    fn eq(&self, other: &str) -> bool {
        self.0 == other
    }
}

impl PartialEq<&str> for NodePath {
    fn eq(&self, other: &&str) -> bool {
        self.0 == *other
    }
}

/// The YAML shape a node was required to have.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ValueKind {
    /// A YAML mapping.
    Mapping,
    /// A YAML sequence.
    Sequence,
    /// A scalar readable as a string.
    String,
    /// A `true`/`false` scalar.
    Boolean,
}

impl fmt::Display for ValueKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Mapping => "a mapping",
            Self::Sequence => "a sequence",
            Self::String => "a string",
            Self::Boolean => "a boolean",
        })
    }
}

/// What went wrong, independent of where it went wrong.
///
/// Match on this instead of on rendered messages: the `Display` text is
/// documentation, the variants are the contract.
#[derive(Clone, Debug, thiserror::Error)]
#[non_exhaustive]
pub enum ErrorKind {
    /// The input is not well-formed YAML. The message comes from the parser.
    #[error("{message}")]
    Syntax {
        /// The parser's description of the syntax error.
        message: String,
    },

    /// The input holds more than one YAML document.
    #[error("a lock file must contain exactly one YAML document")]
    MultipleDocuments,

    /// A mapping defines the same key twice.
    #[error("duplicate mapping key {key:?}")]
    DuplicateKey {
        /// The repeated key.
        key: String,
    },

    /// A node has a shape the schema does not allow here.
    #[error("expected {0}")]
    UnexpectedType(ValueKind),

    /// A required field is absent.
    #[error("missing required field {field:?}")]
    MissingField {
        /// The absent field.
        field: &'static str,
    },

    /// A field is present that this schema version does not define. Unknown
    /// fields are rejected because they may change what a lock file installs.
    #[error("unknown field {field:?}")]
    UnknownField {
        /// The unexpected field.
        field: String,
    },

    /// The `version` field names a schema this crate cannot read.
    #[error("unsupported lock-file version; expected integer 1")]
    UnsupportedVersion,

    /// A package names an installer other than `conda` or `pip`.
    #[error("unknown package manager {found:?}; expected \"conda\" or \"pip\"")]
    UnknownManager {
        /// The manager as written.
        found: String,
    },

    /// A package source uses a type other than `url`.
    #[error("unsupported package source type {found:?}; expected \"url\"")]
    UnsupportedSourceType {
        /// The source type as written.
        found: String,
    },

    /// A declared target platform is not a CEP-26 subdir.
    #[error("expected a CEP-26 target subdir (os-arch), excluding noarch")]
    InvalidPlatform,

    /// The same target platform is declared twice.
    #[error("duplicate target platform")]
    DuplicatePlatform,

    /// A declared target platform has no `content_hash` entry.
    #[error("missing content hash for target platform {platform}")]
    MissingContentHash {
        /// The target platform without a hash.
        platform: String,
    },

    /// A `content_hash` entry names a platform that is not a declared target.
    #[error("content hash refers to an undeclared target platform")]
    UndeclaredContentHashPlatform,

    /// A channel has an empty URL or name.
    #[error("channel URL or name must not be empty")]
    EmptyChannel,

    /// A source is absolute, a URL, or otherwise not relative to the lock file.
    #[error("source paths must be nonempty and relative to the lock file")]
    InvalidSourcePath,

    /// The same source path is declared twice.
    #[error("duplicate source path")]
    DuplicateSource,

    /// `created_at` is not a whole-second UTC timestamp.
    #[error("expected a UTC timestamp in YYYY-MM-DDTHH:MM:SSZ form")]
    InvalidTimestamp,

    /// An `inputs_metadata` entry names a source that is not declared.
    #[error("input hash refers to an undeclared source")]
    UndeclaredInputSource,

    /// A declared source has no `inputs_metadata` entry, although the lock file
    /// records hashes for its other sources.
    #[error("missing input hashes for source {source_path}")]
    MissingInputHashes {
        /// The declared source that has no hashes.
        source_path: String,
    },

    /// A package or dependency name is invalid for its installer.
    #[error("invalid {manager} package name")]
    InvalidPackageName {
        /// The installer whose naming rules apply.
        manager: Manager,
    },

    /// A resolved version is invalid for its installer.
    #[error("invalid resolved {manager} package version")]
    InvalidVersion {
        /// The installer whose version syntax applies.
        manager: Manager,
    },

    /// A package targets a platform that is not declared in the metadata.
    #[error("package target is not declared in metadata.platforms")]
    UndeclaredPackagePlatform,

    /// A package has an empty install category.
    #[error("package category must not be empty")]
    EmptyCategory,

    /// Two packages share one install identity, so the file does not describe a
    /// single installable set.
    #[error("duplicate (name, manager, platform, category) package identity")]
    DuplicatePackageIdentity,

    /// A URL is relative, empty, or contains whitespace.
    #[error("expected an absolute URL without whitespace")]
    InvalidUrl,

    /// A checksum is not a hexadecimal digest of the expected length.
    #[error("expected exactly {length} hexadecimal digits")]
    InvalidDigest {
        /// The digest length the algorithm requires.
        length: usize,
    },

    /// A direct-source package records something other than a revision where an
    /// artifact would record a digest.
    #[error("expected up to 64 hexadecimal digits")]
    InvalidRevision,

    /// A conda build string contains characters conda does not allow.
    #[error("invalid conda build string")]
    InvalidBuildString,

    /// A dependency constraint is invalid for its installer.
    #[error("invalid {manager} dependency constraint")]
    InvalidDependencyConstraint {
        /// The installer whose constraint syntax applies.
        manager: Manager,
    },

    /// The model could not be emitted as YAML.
    #[error("could not serialize the lock file: {message}")]
    Serialize {
        /// The emitter's description of the failure.
        message: String,
    },

    /// A lock file could not be read.
    #[error("could not read {}: {error}", path.display())]
    Read {
        /// The path that was read.
        path: PathBuf,
        /// The underlying I/O error.
        error: Arc<std::io::Error>,
    },

    /// A lock file could not be written.
    #[error("could not write {}: {error}", path.display())]
    Write {
        /// The path that was written.
        path: PathBuf,
        /// The underlying I/O error.
        error: Arc<std::io::Error>,
    },
}

/// A primary or related location in a diagnostic.
#[derive(Clone, Debug)]
pub struct Label {
    pub(crate) path: NodePath,
    pub(crate) message: Option<&'static str>,
    pub(crate) span: Option<Range<usize>>,
}

impl Label {
    /// The model path this label points at.
    pub fn path(&self) -> &NodePath {
        &self.path
    }

    /// Why this location matters. Absent on the primary label, whose
    /// explanation is the diagnostic's [`ErrorKind`].
    pub fn message(&self) -> Option<&'static str> {
        self.message
    }

    /// The UTF-8 byte range in the original source, if known.
    pub fn span(&self) -> Option<Range<usize>> {
        self.span.clone()
    }
}

/// An [`ErrorKind`] with the locations it applies to.
///
/// Errors raised from a model carry model paths only. Parsing a
/// [`crate::Document`], or passing a model error through
/// [`crate::Document::contextualize`], adds the source text and byte spans.
pub type Error = Diagnostic<ErrorKind>;

/// A typed diagnostic that can be located in a CEP-37 document.
///
/// Conversion layers reuse this envelope with their own kind, so a
/// [`crate::Document`] can attach source spans to errors it knows nothing
/// about; see [`crate::Document::contextualize`].
#[derive(Clone, Debug)]
pub struct Diagnostic<K> {
    pub(crate) kind: K,
    pub(crate) labels: Vec<Label>,
    pub(crate) source: Option<Arc<str>>,
    pub(crate) name: Option<Arc<str>>,
    #[cfg(feature = "miette")]
    pub(crate) diagnostic_source: Option<Arc<miette::NamedSource<Arc<str>>>>,
}

impl<K> Diagnostic<K> {
    /// Construct a diagnostic at a model path, without source information.
    pub fn new(path: impl Into<NodePath>, kind: K) -> Self {
        Self {
            kind,
            labels: vec![Label {
                path: path.into(),
                message: None,
                span: None,
            }],
            source: None,
            name: None,
            #[cfg(feature = "miette")]
            diagnostic_source: None,
        }
    }

    /// Add a related model path, for example the first of two duplicate entries.
    #[must_use]
    pub fn with_related_path(mut self, path: impl Into<NodePath>, message: &'static str) -> Self {
        self.labels.push(Label {
            path: path.into(),
            message: Some(message),
            span: None,
        });
        self
    }

    /// What went wrong.
    pub fn kind(&self) -> &K {
        &self.kind
    }

    /// Take the classification, dropping the locations.
    pub fn into_kind(self) -> K {
        self.kind
    }

    /// Reclassify without losing locations or source context. Conversion layers
    /// use this to wrap a lock-file error in their own kind.
    pub fn map_kind<T>(self, map: impl FnOnce(K) -> T) -> Diagnostic<T> {
        Diagnostic {
            kind: map(self.kind),
            labels: self.labels,
            source: self.source,
            name: self.name,
            #[cfg(feature = "miette")]
            diagnostic_source: self.diagnostic_source,
        }
    }

    /// The primary model path. The root path denotes the whole document.
    pub fn path(&self) -> &NodePath {
        &self.labels[0].path
    }

    /// Primary and related labels, the primary one first.
    pub fn labels(&self) -> &[Label] {
        &self.labels
    }

    /// The span of the primary label, if source context is available.
    pub fn span(&self) -> Option<Range<usize>> {
        self.labels[0].span.clone()
    }

    /// Original YAML text, if this diagnostic has document context.
    pub fn source_text(&self) -> Option<&str> {
        self.source.as_deref()
    }

    /// Original file name, if the document was read from a path.
    pub fn source_name(&self) -> Option<&str> {
        self.name.as_deref()
    }

    pub(crate) fn with_primary_span(mut self, span: Option<Range<usize>>) -> Self {
        self.labels[0].span = span;
        self
    }

    pub(crate) fn with_related_span(mut self, span: Option<Range<usize>>) -> Self {
        if let Some(label) = self.labels.last_mut() {
            label.span = span;
        }
        self
    }

    pub(crate) fn attach_source(&mut self, source: Arc<str>, name: Option<Arc<str>>) {
        #[cfg(feature = "miette")]
        {
            self.diagnostic_source = Some(Arc::new(miette::NamedSource::new(
                name.as_deref().unwrap_or("conda-lock.yml"),
                source.clone(),
            )));
        }
        self.source = Some(source);
        self.name = name;
    }
}

impl<K: fmt::Display> fmt::Display for Diagnostic<K> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if let Some(name) = self.source_name() {
            write!(f, "{name}: ")?;
        }
        if !self.path().is_root() {
            write!(f, "{}: ", self.path())?;
        }
        write!(f, "{}", self.kind)
    }
}

impl<K: fmt::Display + fmt::Debug> std::error::Error for Diagnostic<K> {}

#[cfg(feature = "miette")]
impl<K: fmt::Display + fmt::Debug> miette::Diagnostic for Diagnostic<K> {
    fn source_code(&self) -> Option<&dyn miette::SourceCode> {
        self.diagnostic_source
            .as_deref()
            .map(|source| source as &dyn miette::SourceCode)
    }

    fn labels(&self) -> Option<Box<dyn Iterator<Item = miette::LabeledSpan> + '_>> {
        let primary = self.kind.to_string();
        Some(Box::new(self.labels.iter().filter_map(move |label| {
            let message = label.message.map_or_else(|| primary.clone(), str::to_owned);
            label
                .span
                .clone()
                .map(|span| miette::LabeledSpan::new_with_span(Some(message), span))
        })))
    }
}
