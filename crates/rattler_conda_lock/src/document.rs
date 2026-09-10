use std::{collections::BTreeMap, ops::Range, path::Path, str::FromStr, sync::Arc};

use crate::error::Label;
use crate::{Error, LockFile};

#[derive(Clone, Debug)]
pub(crate) struct NodeSpan {
    pub(crate) referenced: Option<Range<usize>>,
    pub(crate) defined: Option<Range<usize>>,
    pub(crate) key: Option<Range<usize>>,
    pub(crate) parent: Option<String>,
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
    /// ```
    /// use rattler_conda_lock::Document;
    ///
    /// let document = Document::parse(
    ///     "version: 1\nmetadata:\n  content_hash: {}\n  channels: []\n  platforms: []\n  sources: []\npackage: []\n",
    /// )?;
    /// assert!(document.lock_file().package.is_empty());
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
            let mut error = Error::new("", format!("could not read lock file: {error}"));
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
    /// Attach this document's original source and locations to a model error.
    ///
    /// ```
    /// use rattler_conda_lock::{Document, Error};
    ///
    /// let document = Document::parse(
    ///     "metadata:\n  content_hash: {}\n  channels: []\n  platforms: []\n  sources: [environment.yml]\npackage: []\n",
    /// )?;
    /// let error = document.contextualize(Error::new(
    ///     "metadata.sources[0]", "this input is unavailable",
    /// ));
    /// let span = error.labels()[0].span().unwrap();
    /// assert_eq!(&document.source_text()[span], "environment.yml");
    /// # Ok::<(), rattler_conda_lock::Error>(())
    /// ```
    pub fn contextualize(&self, mut error: Error) -> Error {
        contextualize_spans(&mut error, &self.spans);
        error.attach_source(self.source.clone(), self.name.clone());
        error
    }
}

pub(crate) fn contextualize_spans(error: &mut Error, spans: &SourceMap) {
    let mut definitions = Vec::new();
    for label in &mut error.labels {
        if label.span.is_some() {
            continue;
        }
        let mut path = label.path.as_str();
        let found = loop {
            if let Some(span) = spans.get(path) {
                if span.referenced.is_some() {
                    break Some(span);
                }
                if let Some(parent) = span.parent.as_deref() {
                    path = parent;
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
        if let Some(span) = found {
            label.span = span.referenced.clone();
            if span.defined.is_some() && span.defined != span.referenced {
                definitions.push(Label {
                    path: label.path.clone(),
                    message: "value defined here (referenced through a YAML alias)".into(),
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
