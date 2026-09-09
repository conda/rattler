use std::{fmt, ops::Range, sync::Arc};

/// A primary or related location in a lock-file diagnostic.
#[derive(Clone, Debug)]
pub struct Label {
    pub(crate) path: String,
    pub(crate) message: String,
    pub(crate) span: Option<Range<usize>>,
}

impl Label {
    /// The model path associated with this label.
    pub fn path(&self) -> &str {
        &self.path
    }
    /// The explanation associated with this location.
    pub fn message(&self) -> &str {
        &self.message
    }
    /// The UTF-8 byte range in the original source, if known.
    pub fn span(&self) -> Option<Range<usize>> {
        self.span.clone()
    }
}

/// A structural, semantic, conversion, or input/output error.
///
/// Errors created from a model have paths but no source locations. Parsing a
/// [`crate::Document`] or calling its `contextualize` method adds source labels.
#[derive(Clone, Debug)]
pub struct Error {
    pub(crate) labels: Vec<Label>,
    pub(crate) source: Option<Arc<str>>,
    pub(crate) name: Option<Arc<str>>,
    #[cfg(feature = "miette")]
    pub(crate) diagnostic_source: Option<Arc<miette::NamedSource<Arc<str>>>>,
}

impl Error {
    /// Construct an error at a model path, without source information.
    pub fn new(path: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            labels: vec![Label {
                path: path.into(),
                message: message.into(),
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
    pub fn with_related_path(
        mut self,
        path: impl Into<String>,
        message: impl Into<String>,
    ) -> Self {
        self.labels.push(Label {
            path: path.into(),
            message: message.into(),
            span: None,
        });
        self
    }

    /// The primary model path. An empty path denotes the whole document.
    pub fn path(&self) -> &str {
        &self.labels[0].path
    }
    /// The primary explanation.
    pub fn message(&self) -> &str {
        &self.labels[0].message
    }
    /// Primary and related labels, including any alias definitions.
    pub fn labels(&self) -> &[Label] {
        &self.labels
    }
    /// Original YAML text, if this error has document context.
    pub fn source_text(&self) -> Option<&str> {
        self.source.as_deref()
    }
    /// Original file name, if parsed from a path.
    pub fn source_name(&self) -> Option<&str> {
        self.name.as_deref()
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

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if let Some(name) = self.source_name() {
            write!(f, "{name}: ")?;
        }
        if !self.path().is_empty() {
            write!(f, "{}: ", self.path())?;
        }
        f.write_str(self.message())
    }
}

impl std::error::Error for Error {}

#[cfg(feature = "miette")]
impl miette::Diagnostic for Error {
    fn source_code(&self) -> Option<&dyn miette::SourceCode> {
        self.diagnostic_source
            .as_deref()
            .map(|source| source as &dyn miette::SourceCode)
    }

    fn labels(&self) -> Option<Box<dyn Iterator<Item = miette::LabeledSpan> + '_>> {
        Some(Box::new(self.labels.iter().filter_map(|label| {
            label
                .span
                .clone()
                .map(|span| miette::LabeledSpan::new_with_span(Some(label.message.clone()), span))
        })))
    }
}
