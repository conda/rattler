use std::{collections::BTreeMap, ops::Range};

use serde::de::{MapAccess, SeqAccess};
use serde::{Deserialize, Deserializer};
use serde_saphyr::{Location, Spanned};
use serde_untagged::UntaggedEnumVisitor;

use crate::document::{NodeSpan, SourceMap, locate_labels};
use crate::error::{ErrorKind, NodePath, ValueKind};
use crate::model::{
    Channel, GitMetadata, Hashes, Manager, Metadata, Package, PackageSource, TimeMetadata,
};
use crate::{Error, LockFile};

// Deserializing into an untagged value tree is intentional: serde's buffered
// enum/flatten machinery discards Spanned locations, while `serde-untagged`
// dispatches on the incoming YAML node without buffering it. Every nested node
// and mapping key therefore stays spanned.
#[derive(Debug)]
enum Raw {
    Null,
    String(String),
    Number(String),
    Bool(bool),
    Sequence(Vec<Spanned<Raw>>),
    Mapping(Vec<(Spanned<String>, Spanned<Raw>)>),
}

impl<'de> Deserialize<'de> for Raw {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        UntaggedEnumVisitor::new()
            .expecting("a YAML value")
            .unit(|| Ok(Raw::Null))
            .bool(|value| Ok(Raw::Bool(value)))
            .i64(|value| Ok(Raw::Number(value.to_string())))
            .u64(|value| Ok(Raw::Number(value.to_string())))
            .f64(|value| Ok(Raw::Number(value.to_string())))
            .string(|value| Ok(Raw::String(value.to_owned())))
            .seq(|mut sequence| {
                let mut values = Vec::new();
                while let Some(value) = sequence.next_element()? {
                    values.push(value);
                }
                Ok(Raw::Sequence(values))
            })
            .map(|mut mapping| {
                // Entries stay a Vec so a duplicate key can be reported with
                // both of its source locations; a map would drop one of them.
                let mut values = Vec::new();
                while let Some(entry) = mapping.next_entry()? {
                    values.push(entry);
                }
                Ok(Raw::Mapping(values))
            })
            .deserialize(deserializer)
    }
}

fn byte_span(location: Location) -> Option<Range<usize>> {
    if location.line() == 0 {
        return None;
    }
    let span = location.span();
    let start = usize::try_from(span.byte_offset()?).ok()?;
    let len = usize::try_from(span.byte_len()?).ok()?;
    Some(start..start.checked_add(len)?)
}

fn index_spans(
    node: &Spanned<Raw>,
    path: NodePath,
    parent: Option<NodePath>,
    key: Option<Range<usize>>,
    spans: &mut SourceMap,
) -> Result<(), Error> {
    spans.insert(
        path.as_str().to_owned(),
        NodeSpan {
            referenced: byte_span(node.referenced),
            defined: byte_span(node.defined),
            key,
            parent,
        },
    );
    match &node.value {
        Raw::Mapping(values) => {
            let mut keys = BTreeMap::new();
            for (key, value) in values {
                let child = path.field(&key.value);
                if let Some(first) = keys.insert(&key.value, key) {
                    return Err(Error::new(
                        child.clone(),
                        ErrorKind::DuplicateKey {
                            key: key.value.clone(),
                        },
                    )
                    .with_primary_span(byte_span(key.referenced))
                    .with_related_path(child, "first defined here")
                    .with_related_span(byte_span(first.referenced)));
                }
                index_spans(
                    value,
                    child,
                    Some(path.clone()),
                    byte_span(key.referenced),
                    spans,
                )?;
            }
        }
        Raw::Sequence(values) => {
            for (index, value) in values.iter().enumerate() {
                index_spans(value, path.index(index), Some(path.clone()), None, spans)?;
            }
        }
        _ => {}
    }
    Ok(())
}

/// serde-saphyr appends its location to messages that already end in it; keep
/// one copy so a reported syntax error names its position once.
fn dedupe_location(message: &str) -> String {
    if let Some(index) = message.rfind(" at line ") {
        let (head, tail) = message.split_at(index);
        if head.ends_with(tail) {
            return head.to_owned();
        }
    }
    message.to_owned()
}

pub(crate) fn parse(source: &str) -> Result<(LockFile, SourceMap), Error> {
    // Pass duplicate pairs through to our Vec-backed value tree, then reject
    // them with the complete model path and both mapping-key source locations.
    let options = serde_saphyr::options! {
        duplicate_keys: serde_saphyr::DuplicateKeyPolicy::LastWins,
    };
    let raw: Spanned<Raw> = serde_saphyr::from_str_with_options(source, options).map_err(
        |error: serde_saphyr::Error| {
            let span = error.location().and_then(byte_span);
            // The underlying message points at multi-document parser entry
            // points that this crate deliberately does not expose.
            let reported = dedupe_location(&error.without_snippet().to_string());
            let kind = if reported.contains("multiple YAML documents") {
                ErrorKind::MultipleDocuments
            } else {
                ErrorKind::Syntax { message: reported }
            };
            Error::new(NodePath::root(), kind).with_primary_span(span)
        },
    )?;
    let mut spans = SourceMap::new();
    index_spans(&raw, NodePath::root(), None, None, &mut spans)?;
    let lock_file = Parser { source }.lock_file(raw).map_err(|mut error| {
        locate_labels(&mut error.labels, &spans);
        error
    })?;
    Ok((lock_file, spans))
}

struct Fields {
    path: NodePath,
    values: BTreeMap<String, Spanned<Raw>>,
}

impl Fields {
    fn new(node: Spanned<Raw>, path: NodePath) -> Result<Self, Error> {
        let Raw::Mapping(values) = node.value else {
            return Err(Error::new(
                path,
                ErrorKind::UnexpectedType(ValueKind::Mapping),
            ));
        };
        Ok(Self {
            path,
            values: values
                .into_iter()
                .map(|(key, value)| (key.value, value))
                .collect(),
        })
    }
    fn required(&mut self, field: &'static str) -> Result<Spanned<Raw>, Error> {
        self.values
            .remove(field)
            .ok_or_else(|| Error::new(self.path.field(field), ErrorKind::MissingField { field }))
    }
    fn optional(&mut self, field: &str) -> Option<Spanned<Raw>> {
        self.values
            .remove(field)
            .filter(|node| !matches!(node.value, Raw::Null))
    }
    fn finish(self) -> Result<(), Error> {
        if let Some((field, _)) = self.values.into_iter().next() {
            Err(Error::new(
                self.path.field(&field),
                ErrorKind::UnknownField { field },
            ))
        } else {
            Ok(())
        }
    }
}

fn sequence(node: Spanned<Raw>, path: &NodePath) -> Result<Vec<Spanned<Raw>>, Error> {
    match node.value {
        Raw::Sequence(values) => Ok(values),
        _ => Err(Error::new(
            path.clone(),
            ErrorKind::UnexpectedType(ValueKind::Sequence),
        )),
    }
}

struct Parser<'a> {
    source: &'a str,
}
impl Parser<'_> {
    fn string(&self, node: Spanned<Raw>, path: &NodePath) -> Result<String, Error> {
        match node.value {
            Raw::String(value) => Ok(value),
            Raw::Number(value) => {
                // YAML's string coercion is useful for unquoted package versions;
                // preserve the original spelling, not an f64 reformatting of it.
                Ok(byte_span(node.defined)
                    .and_then(|span| self.source.get(span))
                    .unwrap_or(&value)
                    .to_owned())
            }
            Raw::Bool(value) => Ok(byte_span(node.defined)
                .and_then(|span| self.source.get(span))
                .unwrap_or(if value { "true" } else { "false" })
                .to_owned()),
            _ => Err(Error::new(
                path.clone(),
                ErrorKind::UnexpectedType(ValueKind::String),
            )),
        }
    }
    fn strings(&self, node: Spanned<Raw>, path: &NodePath) -> Result<Vec<String>, Error> {
        sequence(node, path)?
            .into_iter()
            .enumerate()
            .map(|(index, node)| self.string(node, &path.index(index)))
            .collect()
    }
    fn string_field(&self, fields: &mut Fields, field: &'static str) -> Result<String, Error> {
        let path = fields.path.field(field);
        self.string(fields.required(field)?, &path)
    }
    fn optional_string(
        &self,
        fields: &mut Fields,
        field: &'static str,
    ) -> Result<Option<String>, Error> {
        let path = fields.path.field(field);
        fields
            .optional(field)
            .map(|node| self.string(node, &path))
            .transpose()
    }
    fn string_map(
        &self,
        node: Spanned<Raw>,
        path: &NodePath,
    ) -> Result<BTreeMap<String, String>, Error> {
        Fields::new(node, path.clone())?
            .values
            .into_iter()
            .map(|(key, value)| {
                self.string(value, &path.field(&key))
                    .map(|value| (key, value))
            })
            .collect()
    }
    fn lock_file(&self, node: Spanned<Raw>) -> Result<LockFile, Error> {
        let mut fields = Fields::new(node, NodePath::root())?;
        let version_path = NodePath::root().field("version");
        if let Some(version) = fields.values.remove("version")
            && (!matches!(&version.value, Raw::Number(value) if value == "1")
                || self.string(version, &version_path)? != "1")
        {
            return Err(Error::new(version_path, ErrorKind::UnsupportedVersion));
        }
        let metadata = self.metadata(fields.required("metadata")?)?;
        let package_path = NodePath::root().field("package");
        let package = sequence(fields.required("package")?, &package_path)?
            .into_iter()
            .enumerate()
            .map(|(index, node)| self.package(node, &package_path.index(index)))
            .collect::<Result<_, _>>()?;
        fields.finish()?;
        Ok(LockFile { metadata, package })
    }
    fn hashes(&self, node: Spanned<Raw>, path: &NodePath) -> Result<Hashes, Error> {
        let mut fields = Fields::new(node, path.clone())?;
        let md5 = self.optional_string(&mut fields, "md5")?;
        let sha256 = self.optional_string(&mut fields, "sha256")?;
        fields.finish()?;
        Ok(Hashes { md5, sha256 })
    }
    fn channel(&self, node: Spanned<Raw>, path: &NodePath) -> Result<Channel, Error> {
        if matches!(node.value, Raw::String(_)) {
            return Ok(Channel {
                url: self.string(node, path)?,
                used_env_vars: Vec::new(),
            });
        }
        let mut fields = Fields::new(node, path.clone())?;
        let url = self.string_field(&mut fields, "url")?;
        let used_env_vars = self.strings(
            fields.required("used_env_vars")?,
            &path.field("used_env_vars"),
        )?;
        fields.finish()?;
        Ok(Channel { url, used_env_vars })
    }
    fn metadata(&self, node: Spanned<Raw>) -> Result<Metadata, Error> {
        let path = NodePath::root().field("metadata");
        let mut fields = Fields::new(node, path.clone())?;
        let content_hash = self.string_map(
            fields.required("content_hash")?,
            &path.field("content_hash"),
        )?;
        let channels_path = path.field("channels");
        let channels = sequence(fields.required("channels")?, &channels_path)?
            .into_iter()
            .enumerate()
            .map(|(index, node)| self.channel(node, &channels_path.index(index)))
            .collect::<Result<_, _>>()?;
        let platforms = self.strings(fields.required("platforms")?, &path.field("platforms"))?;
        let sources = self.strings(fields.required("sources")?, &path.field("sources"))?;
        let time_metadata = fields
            .optional("time_metadata")
            .map(|node| {
                let mut fields = Fields::new(node, path.field("time_metadata"))?;
                let created_at = self.string_field(&mut fields, "created_at")?;
                fields.finish()?;
                Ok::<_, Error>(TimeMetadata { created_at })
            })
            .transpose()?;
        let git_metadata = fields
            .optional("git_metadata")
            .map(|node| {
                let mut fields = Fields::new(node, path.field("git_metadata"))?;
                let git_user_name = self.optional_string(&mut fields, "git_user_name")?;
                let git_user_email = self.optional_string(&mut fields, "git_user_email")?;
                let git_sha = self.optional_string(&mut fields, "git_sha")?;
                fields.finish()?;
                Ok::<_, Error>(GitMetadata {
                    git_user_name,
                    git_user_email,
                    git_sha,
                })
            })
            .transpose()?;
        let inputs_metadata = fields
            .optional("inputs_metadata")
            .map(|node| {
                let inputs_path = path.field("inputs_metadata");
                Fields::new(node, inputs_path.clone())?
                    .values
                    .into_iter()
                    .map(|(key, node)| {
                        self.hashes(node, &inputs_path.field(&key))
                            .map(|hash| (key, hash))
                    })
                    .collect::<Result<BTreeMap<_, _>, Error>>()
            })
            .transpose()?;
        let custom_metadata = fields
            .optional("custom_metadata")
            .map(|node| self.string_map(node, &path.field("custom_metadata")))
            .transpose()?;
        fields.finish()?;
        Ok(Metadata {
            content_hash,
            channels,
            platforms,
            sources,
            time_metadata,
            git_metadata,
            inputs_metadata,
            custom_metadata,
        })
    }
    fn package(&self, node: Spanned<Raw>, path: &NodePath) -> Result<Package, Error> {
        let mut fields = Fields::new(node, path.clone())?;
        let name = self.string_field(&mut fields, "name")?;
        let version = self.string_field(&mut fields, "version")?;
        let manager = match self.string_field(&mut fields, "manager")?.as_str() {
            "conda" => Manager::Conda,
            "pip" => Manager::Pip,
            found => {
                return Err(Error::new(
                    path.field("manager"),
                    ErrorKind::UnknownManager {
                        found: found.to_owned(),
                    },
                ));
            }
        };
        let platform = self.string_field(&mut fields, "platform")?;
        let dependencies = fields
            .optional("dependencies")
            .map(|node| self.string_map(node, &path.field("dependencies")))
            .transpose()?
            .unwrap_or_default();
        let url = self.string_field(&mut fields, "url")?;
        let hash = self.hashes(fields.required("hash")?, &path.field("hash"))?;
        let source = fields
            .optional("source")
            .map(|node| {
                let source_path = path.field("source");
                let mut fields = Fields::new(node, source_path.clone())?;
                let found = self.string_field(&mut fields, "type")?;
                if found != "url" {
                    return Err(Error::new(
                        source_path.field("type"),
                        ErrorKind::UnsupportedSourceType { found },
                    ));
                }
                let url = self.string_field(&mut fields, "url")?;
                fields.finish()?;
                Ok(PackageSource { url })
            })
            .transpose()?;
        let build = self.optional_string(&mut fields, "build")?;
        // Category has a schema default; an explicit null is not a category.
        let category = fields
            .values
            .remove("category")
            .map(|node| self.string(node, &path.field("category")))
            .transpose()?
            .unwrap_or_else(|| "main".into());
        let optional = match fields.required("optional")?.value {
            Raw::Bool(value) => value,
            _ => {
                return Err(Error::new(
                    path.field("optional"),
                    ErrorKind::UnexpectedType(ValueKind::Boolean),
                ));
            }
        };
        fields.finish()?;
        Ok(Package {
            name,
            version,
            manager,
            platform,
            dependencies,
            url,
            hash,
            source,
            build,
            category,
            optional,
        })
    }
}

#[cfg(test)]
mod tests {
    use crate::{Document, Error, ErrorKind};

    const EMPTY: &str = "metadata:\n  content_hash: {}\n  channels: []\n  platforms: []\n  sources: []\npackage: []\n";

    #[test]
    fn unicode_crlf_and_dotted_keys_retain_byte_locations() {
        let source = EMPTY
            .replace(
                "  sources: []",
                "  sources: []\n  custom_metadata:\n    clé.with.dots: héllo",
            )
            .replace('\n', "\r\n");
        let document = Document::parse(&source).unwrap();
        let mut error = Error::new(
            "metadata.custom_metadata.clé.with.dots",
            ErrorKind::InvalidUrl,
        );
        document.locate(&mut error.labels);
        assert_eq!(&source[error.span().unwrap()], "héllo");
        let key = document
            .key_span("metadata.custom_metadata.clé.with.dots")
            .unwrap();
        assert_eq!(&source[key], "clé.with.dots");
    }

    #[test]
    fn alias_diagnostics_retain_use_and_definition() {
        let source = EMPTY.replace(
            "  sources: []",
            "  sources: []\n  custom_metadata:\n    original: &value héllo\n    copied: *value",
        );
        let document = Document::parse(&source).unwrap();
        let mut error = Error::new("metadata.custom_metadata.copied", ErrorKind::InvalidUrl);
        document.locate(&mut error.labels);
        assert_eq!(&source[error.span().unwrap()], "*value");
        assert!(
            error
                .labels()
                .iter()
                .skip(1)
                .any(|label| source[label.span().unwrap()].contains("héllo"))
        );
    }

    #[test]
    fn missing_fields_use_parent_and_unknown_fields_are_rejected() {
        let source = EMPTY.replace(
            "  channels: []",
            "  channels:\n    - url: https://example.com",
        );
        let error = Document::parse(&source).unwrap_err();
        assert_eq!(error.path().as_str(), "metadata.channels[0].used_env_vars");
        assert!(matches!(
            error.kind(),
            ErrorKind::MissingField {
                field: "used_env_vars"
            }
        ));
        assert!(error.span().is_some());
        let source = format!("{EMPTY}future_semantics: true\n");
        let error = Document::parse(&source).unwrap_err();
        assert_eq!(error.path().as_str(), "future_semantics");
        assert!(matches!(error.kind(), ErrorKind::UnknownField { .. }));
        assert!(error.span().is_some());
    }

    #[test]
    fn duplicate_keys_report_both_definitions() {
        let source = EMPTY.replace(
            "  sources: []",
            "  sources: []\n  custom_metadata: {key: first, key: second}",
        );
        let error = Document::parse(&source).unwrap_err();
        assert_eq!(error.path().as_str(), "metadata.custom_metadata.key");
        assert!(matches!(error.kind(), ErrorKind::DuplicateKey { .. }));
        assert_eq!(error.labels().len(), 2);
        let first = error.labels()[0].span().unwrap();
        let second = error.labels()[1].span().unwrap();
        assert_eq!(&source[first.clone()], "key");
        assert_eq!(&source[second.clone()], "key");
        assert_ne!(first, second);
    }
}
