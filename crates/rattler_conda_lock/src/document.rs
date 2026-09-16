use std::{collections::BTreeMap, ops::Range, path::Path, str::FromStr, sync::Arc};

use crate::error::{Diagnostic, ErrorKind, Label, NodePath};
use crate::{Error, LockFile};

#[derive(Clone, Debug)]
pub(crate) struct NodeSpan {
    pub(crate) referenced: Option<Range<usize>>,
    pub(crate) defined: Option<Range<usize>>,
    pub(crate) key: Option<Range<usize>>,
    pub(crate) parent: Option<NodePath>,
}

pub(crate) type SourceMap = BTreeMap<String, NodeSpan>;

/// An immutable lock file together with its original YAML source locations.
///
/// Borrow the model with [`Self::lock_file`] for source-aware conversions. To edit
/// it, consume the document with [`Self::into_lock_file`]; source locations cannot
/// then accidentally refer to a changed model.
#[derive(Clone, Debug)]
pub struct Document {
    lock_file: LockFile,
    source: Arc<str>,
    name: Option<Arc<str>>,
    spans: SourceMap,
}

impl Document {
    /// Parse and validate a single CEP-37 v1 YAML document.
    ///
    /// Every error this returns already carries the source text and the span of
    /// the offending node.
    ///
    /// ```
    /// use rattler_conda_lock::{Document, Manager};
    ///
    /// let source = "\
    /// version: 1
    /// metadata:
    ///   content_hash:
    ///     linux-64: 8b7df143d91c716ecfa5fc1730022f6b421b05cedee8fd52b1fc65a96030ad52
    ///   channels:
    ///     - url: conda-forge
    ///       used_env_vars: []
    ///   platforms: [linux-64]
    ///   sources: [environment.yml]
    /// package:
    ///   - name: python
    ///     version: '3.13.1'
    ///     manager: conda
    ///     platform: linux-64
    ///     dependencies:
    ///       libgcc: '>=13'
    ///     url: https://conda.anaconda.org/conda-forge/linux-64/python-3.13.1-h9e4cc4f_0.conda
    ///     hash:
    ///       sha256: 1b98e5a2b1f5b3e2a6b44f5a5f42c9b1e2a3f5a6b7c8d9e0f1a2b3c4d5e6f708
    ///     category: main
    ///     optional: false
    /// ";
    /// let document = Document::parse(source)?;
    ///
    /// let package = &document.lock_file().package[0];
    /// assert_eq!(package.manager, Manager::Conda);
    /// assert_eq!(package.dependencies["libgcc"], ">=13");
    ///
    /// // The version keeps the spelling of the file, not a YAML float.
    /// assert_eq!(package.version, "3.13.1");
    ///
    /// // Every node can be located in the original text.
    /// let span = document.span("package[0].url").unwrap();
    /// assert!(source[span].ends_with("python-3.13.1-h9e4cc4f_0.conda"));
    /// # Ok::<(), rattler_conda_lock::Error>(())
    /// ```
    pub fn parse(source: &str) -> Result<Self, Error> {
        Self::parse_named(Arc::from(source), None)
    }

    /// Read UTF-8 YAML, retaining the file name in subsequent diagnostics.
    pub fn from_path(path: impl AsRef<Path>) -> Result<Self, Error> {
        let path = path.as_ref();
        let name: Arc<str> = Arc::from(path.to_string_lossy().as_ref());
        let source = std::fs::read_to_string(path).map_err(|error| {
            let mut error = Error::new(
                NodePath::root(),
                ErrorKind::Read {
                    path: path.to_owned(),
                    error: Arc::new(error),
                },
            );
            error.name = Some(name.clone());
            error
        })?;
        Self::parse_named(Arc::from(source), Some(name))
    }

    fn parse_named(source: Arc<str>, name: Option<Arc<str>>) -> Result<Self, Error> {
        let (lock_file, spans) = crate::parse::parse(&source).map_err(|mut error| {
            error.attach_source(source.clone(), name.clone());
            error
        })?;
        let document = Self {
            lock_file,
            source,
            name,
            spans,
        };
        document
            .lock_file
            .validate()
            .map_err(|error| document.contextualize(error))?;
        Ok(document)
    }

    /// Borrow the parsed model without invalidating source locations.
    pub fn lock_file(&self) -> &LockFile {
        &self.lock_file
    }
    /// Consume the source document and return its editable model.
    pub fn into_lock_file(self) -> LockFile {
        self.lock_file
    }
    /// The original, unmodified YAML text.
    pub fn source_text(&self) -> &str {
        &self.source
    }
    /// The input path, when loaded from disk.
    pub fn source_name(&self) -> Option<&str> {
        self.name.as_deref()
    }
    /// The exact value span for a model path, if available.
    pub fn span(&self, path: &str) -> Option<Range<usize>> {
        self.spans
            .get(path)
            .and_then(|span| span.referenced.clone())
    }
    /// The mapping-key span for a model path, if available.
    pub fn key_span(&self, path: &str) -> Option<Range<usize>> {
        self.spans.get(path).and_then(|span| span.key.clone())
    }

    /// Attach this document's source text and locations to a diagnostic raised
    /// elsewhere.
    ///
    /// Everything this crate returns for a document is contextualized already;
    /// so are the conversions in `rattler_lock::conda_lock` that accept a
    /// `Document`. This method exists for diagnostics about a lock file that
    /// this crate cannot produce itself — a caller that resolves
    /// `metadata.sources`, verifies `content_hash`, or maps packages onto its
    /// own model raises its own [`Diagnostic`] kind, and only the document
    /// knows where those paths live in the file.
    ///
    /// Labels that already carry a span are left alone, and a path with no
    /// recorded span falls back to its nearest recorded ancestor, so an error
    /// about a missing field still points at the mapping that lacks it.
    ///
    /// ```
    /// use rattler_conda_lock::{Diagnostic, Document};
    ///
    /// # let source = "metadata:\n  content_hash: {}\n  channels: []\n  platforms: []\n  sources: [environment.yml]\npackage: []\n";
    /// let document = Document::parse(source)?;
    ///
    /// // A caller-defined kind: this crate never reads the source files.
    /// #[derive(Debug, thiserror::Error)]
    /// #[error("input file is no longer present")]
    /// struct MissingInput;
    ///
    /// let error = document.contextualize(Diagnostic::new("metadata.sources[0]", MissingInput));
    /// assert_eq!(&source[error.span().unwrap()], "environment.yml");
    /// # Ok::<(), rattler_conda_lock::Error>(())
    /// ```
    pub fn contextualize<K>(&self, mut error: Diagnostic<K>) -> Diagnostic<K> {
        contextualize_spans(&mut error, &self.spans);
        error.attach_source(self.source.clone(), self.name.clone());
        error
    }
}

pub(crate) fn contextualize_spans<K>(error: &mut Diagnostic<K>, spans: &SourceMap) {
    let mut definitions = Vec::new();
    for label in &mut error.labels {
        if label.span.is_some() {
            continue;
        }
        let mut path = label.path.as_str();
        let found = loop {
            if let Some(span) = spans.get(path) {
                if span.referenced.is_some() {
                    break Some((span, path));
                }
                if let Some(parent) = span.parent.as_ref() {
                    path = parent.as_str();
                    continue;
                }
            }
            if path.is_empty() {
                break None;
            }
            // Exact lookup above handles mapping keys containing dots. Only a
            // missing path is shortened, selecting the longest recorded parent.
            path = match path.rfind(['.', '[']) {
                Some(index) => &path[..index],
                None => "",
            };
        };
        if let Some((span, resolved)) = found {
            // A block collection spans from its first child, so its own value
            // span is empty; the mapping key that introduced it is the node a
            // reader recognizes. Falling back to the document as a whole points
            // at nothing, so a path the document does not contain at all — a
            // caller's own option, say — keeps no span.
            let resolved_document = resolved.is_empty() && !label.path.is_root();
            label.span = span
                .referenced
                .clone()
                .filter(|range| !range.is_empty())
                .or_else(|| span.key.clone())
                .or_else(|| {
                    (!resolved_document)
                        .then(|| span.referenced.clone())
                        .flatten()
                });
            if label.span.is_some() && span.defined.is_some() && span.defined != span.referenced {
                definitions.push(Label {
                    path: label.path.clone(),
                    message: Some("value defined here (referenced through a YAML alias)"),
                    span: span.defined.clone(),
                });
            }
        }
    }
    error.labels.extend(definitions);
}

impl FromStr for LockFile {
    type Err = Error;
    fn from_str(source: &str) -> Result<Self, Self::Err> {
        Document::parse(source).map(Document::into_lock_file)
    }
}
