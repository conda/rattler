use std::{collections::BTreeMap, fmt, ops::Range};

use serde::de::{self, MapAccess, SeqAccess, Visitor};
use serde::{Deserialize, Deserializer};
use serde_saphyr::{Location, Spanned};

use crate::document::{NodeSpan, SourceMap, contextualize_spans};
use crate::model::{
    Channel, GitMetadata, Hashes, Manager, Metadata, Package, PackageSource, TimeMetadata,
};
use crate::{Error, LockFile};

// A direct visitor is intentional: serde's buffered enum/flatten machinery
// discards Spanned locations. Every nested node and mapping key stays spanned.
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
        struct RawVisitor;
        impl<'de> Visitor<'de> for RawVisitor {
            type Value = Raw;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("a YAML value")
            }
            fn visit_unit<E: de::Error>(self) -> Result<Raw, E> {
                Ok(Raw::Null)
            }
            fn visit_none<E: de::Error>(self) -> Result<Raw, E> {
                Ok(Raw::Null)
            }
            fn visit_bool<E: de::Error>(self, value: bool) -> Result<Raw, E> {
                Ok(Raw::Bool(value))
            }
            fn visit_str<E: de::Error>(self, value: &str) -> Result<Raw, E> {
                Ok(Raw::String(value.into()))
            }
            fn visit_string<E: de::Error>(self, value: String) -> Result<Raw, E> {
                Ok(Raw::String(value))
            }
            fn visit_i64<E: de::Error>(self, value: i64) -> Result<Raw, E> {
                Ok(Raw::Number(value.to_string()))
            }
            fn visit_u64<E: de::Error>(self, value: u64) -> Result<Raw, E> {
                Ok(Raw::Number(value.to_string()))
            }
            fn visit_f64<E: de::Error>(self, value: f64) -> Result<Raw, E> {
                Ok(Raw::Number(value.to_string()))
            }
            fn visit_seq<A: SeqAccess<'de>>(self, mut sequence: A) -> Result<Raw, A::Error> {
                let mut values = Vec::new();
                while let Some(value) = sequence.next_element()? {
                    values.push(value);
                }
                Ok(Raw::Sequence(values))
            }
            fn visit_map<A: MapAccess<'de>>(self, mut mapping: A) -> Result<Raw, A::Error> {
                let mut values = Vec::new();
                while let Some(entry) = mapping.next_entry()? {
                    values.push(entry);
                }
                Ok(Raw::Mapping(values))
            }
        }
        deserializer.deserialize_any(RawVisitor)
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

fn field_path(parent: &str, field: &str) -> String {
    if parent.is_empty() {
        field.into()
    } else {
        format!("{parent}.{field}")
    }
}

fn index_spans(
    node: &Spanned<Raw>,
    path: String,
    parent: Option<String>,
    key: Option<Range<usize>>,
    spans: &mut SourceMap,
) -> Result<(), Error> {
    spans.insert(
        path.clone(),
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
                let child = field_path(&path, &key.value);
                if let Some(first) = keys.insert(&key.value, key) {
                    let mut error =
                        Error::new(&child, format!("duplicate mapping key {:?}", key.value))
                            .with_related_path(&child, "first defined here");
                    error.labels[0].span = byte_span(key.referenced);
                    error.labels[1].span = byte_span(first.referenced);
                    return Err(error);
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
                index_spans(
                    value,
                    format!("{path}[{index}]"),
                    Some(path.clone()),
                    None,
                    spans,
                )?;
            }
        }
        _ => {}
    }
    Ok(())
}

pub(crate) fn parse(source: &str) -> Result<(LockFile, SourceMap), Error> {
    // Pass duplicate pairs through to our Vec-backed visitor, then reject them
    // with the complete model path and both mapping-key source locations.
    let options = serde_saphyr::options! {
        duplicate_keys: serde_saphyr::DuplicateKeyPolicy::LastWins,
    };
    let raw: Spanned<Raw> = serde_saphyr::from_str_with_options(source, options).map_err(
        |error: serde_saphyr::Error| {
            // The underlying message points at multi-document parser entry
            // points that this crate deliberately does not expose.
            let reported = error.without_snippet().to_string();
            let message = if reported.contains("multiple YAML documents") {
                "a lock file must contain exactly one YAML document".to_owned()
            } else {
                reported
            };
            let mut result = Error::new("", message);
            result.labels[0].span = error.location().and_then(byte_span);
            result
        },
    )?;
    let mut spans = SourceMap::new();
    index_spans(&raw, String::new(), None, None, &mut spans)?;
    let lock_file = Parser { source }.lock_file(raw).map_err(|mut error| {
        contextualize_spans(&mut error, &spans);
        error
    })?;
    Ok((lock_file, spans))
}

struct Fields {
    path: String,
    values: BTreeMap<String, Spanned<Raw>>,
}

impl Fields {
    fn new(node: Spanned<Raw>, path: &str) -> Result<Self, Error> {
        let Raw::Mapping(values) = node.value else {
            return Err(Error::new(path, "expected a mapping"));
        };
        Ok(Self {
            path: path.into(),
            values: values
                .into_iter()
                .map(|(key, value)| (key.value, value))
                .collect(),
        })
    }
    fn required(&mut self, field: &str) -> Result<Spanned<Raw>, Error> {
        self.values.remove(field).ok_or_else(|| {
            Error::new(
                field_path(&self.path, field),
                format!("missing required field {field:?}"),
            )
        })
    }
    fn optional(&mut self, field: &str) -> Option<Spanned<Raw>> {
        self.values
            .remove(field)
            .filter(|node| !matches!(node.value, Raw::Null))
    }
    fn finish(self) -> Result<(), Error> {
        if let Some((field, _)) = self.values.into_iter().next() {
            Err(Error::new(
                field_path(&self.path, &field),
                format!("unknown field {field:?}"),
            ))
        } else {
            Ok(())
        }
    }
}

fn sequence(node: Spanned<Raw>, path: &str) -> Result<Vec<Spanned<Raw>>, Error> {
    match node.value {
        Raw::Sequence(values) => Ok(values),
        _ => Err(Error::new(path, "expected a sequence")),
    }
}

struct Parser<'a> {
    source: &'a str,
}
impl Parser<'_> {
    fn string(&self, node: Spanned<Raw>, path: &str) -> Result<String, Error> {
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
            _ => Err(Error::new(path, "expected a string")),
        }
    }
    fn strings(&self, node: Spanned<Raw>, path: &str) -> Result<Vec<String>, Error> {
        sequence(node, path)?
            .into_iter()
            .enumerate()
            .map(|(index, node)| self.string(node, &format!("{path}[{index}]")))
            .collect()
    }
    fn string_field(&self, fields: &mut Fields, field: &str) -> Result<String, Error> {
        let path = field_path(&fields.path, field);
        self.string(fields.required(field)?, &path)
    }
    fn optional_string(&self, fields: &mut Fields, field: &str) -> Result<Option<String>, Error> {
        let path = field_path(&fields.path, field);
        fields
            .optional(field)
            .map(|node| self.string(node, &path))
            .transpose()
    }
    fn string_map(
        &self,
        node: Spanned<Raw>,
        path: &str,
    ) -> Result<BTreeMap<String, String>, Error> {
        Fields::new(node, path)?
            .values
            .into_iter()
            .map(|(key, value)| {
                self.string(value, &field_path(path, &key))
                    .map(|value| (key, value))
            })
            .collect()
    }
    fn lock_file(&self, node: Spanned<Raw>) -> Result<LockFile, Error> {
        let mut fields = Fields::new(node, "")?;
        if let Some(version) = fields.values.remove("version")
            && (!matches!(&version.value, Raw::Number(value) if value == "1")
                || self.string(version, "version")? != "1")
        {
            return Err(Error::new(
                "version",
                "unsupported lock-file version; expected integer 1",
            ));
        }
        let metadata = self.metadata(fields.required("metadata")?)?;
        let package = sequence(fields.required("package")?, "package")?
            .into_iter()
            .enumerate()
            .map(|(index, node)| self.package(node, &format!("package[{index}]")))
            .collect::<Result<_, _>>()?;
        fields.finish()?;
        Ok(LockFile { metadata, package })
    }
    fn hashes(&self, node: Spanned<Raw>, path: &str) -> Result<Hashes, Error> {
        let mut fields = Fields::new(node, path)?;
        let md5 = self.optional_string(&mut fields, "md5")?;
        let sha256 = self.optional_string(&mut fields, "sha256")?;
        fields.finish()?;
        Ok(Hashes { md5, sha256 })
    }
    fn channel(&self, node: Spanned<Raw>, path: &str) -> Result<Channel, Error> {
        if matches!(node.value, Raw::String(_)) {
            return Ok(Channel {
                url: self.string(node, path)?,
                used_env_vars: Vec::new(),
            });
        }
        let mut fields = Fields::new(node, path)?;
        let url = self.string_field(&mut fields, "url")?;
        let used_env_vars = self.strings(
            fields.required("used_env_vars")?,
            &field_path(path, "used_env_vars"),
        )?;
        fields.finish()?;
        Ok(Channel { url, used_env_vars })
    }
    fn metadata(&self, node: Spanned<Raw>) -> Result<Metadata, Error> {
        let mut fields = Fields::new(node, "metadata")?;
        let content_hash =
            self.string_map(fields.required("content_hash")?, "metadata.content_hash")?;
        let channels = sequence(fields.required("channels")?, "metadata.channels")?
            .into_iter()
            .enumerate()
            .map(|(index, node)| self.channel(node, &format!("metadata.channels[{index}]")))
            .collect::<Result<_, _>>()?;
        let platforms = self.strings(fields.required("platforms")?, "metadata.platforms")?;
        let sources = self.strings(fields.required("sources")?, "metadata.sources")?;
        let time_metadata = fields
            .optional("time_metadata")
            .map(|node| {
                let mut fields = Fields::new(node, "metadata.time_metadata")?;
                let created_at = self.string_field(&mut fields, "created_at")?;
                fields.finish()?;
                Ok::<_, Error>(TimeMetadata { created_at })
            })
            .transpose()?;
        let git_metadata = fields
            .optional("git_metadata")
            .map(|node| {
                let mut fields = Fields::new(node, "metadata.git_metadata")?;
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
                Fields::new(node, "metadata.inputs_metadata")?
                    .values
                    .into_iter()
                    .map(|(key, node)| {
                        self.hashes(node, &field_path("metadata.inputs_metadata", &key))
                            .map(|hash| (key, hash))
                    })
                    .collect::<Result<BTreeMap<_, _>, Error>>()
            })
            .transpose()?;
        let custom_metadata = fields
            .optional("custom_metadata")
            .map(|node| self.string_map(node, "metadata.custom_metadata"))
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
    fn package(&self, node: Spanned<Raw>, path: &str) -> Result<Package, Error> {
        let mut fields = Fields::new(node, path)?;
        let name = self.string_field(&mut fields, "name")?;
        let version = self.string_field(&mut fields, "version")?;
        let manager = match self.string_field(&mut fields, "manager")?.as_str() {
            "conda" => Manager::Conda,
            "pip" => Manager::Pip,
            _ => {
                return Err(Error::new(
                    field_path(path, "manager"),
                    "expected manager conda or pip",
                ));
            }
        };
        let platform = self.string_field(&mut fields, "platform")?;
        let dependencies = fields
            .optional("dependencies")
            .map(|node| self.string_map(node, &field_path(path, "dependencies")))
            .transpose()?
            .unwrap_or_default();
        let url = self.string_field(&mut fields, "url")?;
        let hash = self.hashes(fields.required("hash")?, &field_path(path, "hash"))?;
        let source = fields
            .optional("source")
            .map(|node| {
                let source_path = field_path(path, "source");
                let mut fields = Fields::new(node, &source_path)?;
                if self.string_field(&mut fields, "type")? != "url" {
                    return Err(Error::new(
                        field_path(&source_path, "type"),
                        "unsupported package source type; expected url",
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
            .map(|node| self.string(node, &field_path(path, "category")))
            .transpose()?
            .unwrap_or_else(|| "main".into());
        let optional = match fields.required("optional")?.value {
            Raw::Bool(value) => value,
            _ => {
                return Err(Error::new(
                    field_path(path, "optional"),
                    "expected a boolean",
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
    use crate::{Document, Error};

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
        let error = document.contextualize(Error::new(
            "metadata.custom_metadata.clé.with.dots",
            "invalid value",
        ));
        assert_eq!(&source[error.labels()[0].span().unwrap()], "héllo");
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
        let error = document.contextualize(Error::new(
            "metadata.custom_metadata.copied",
            "invalid value",
        ));
        assert_eq!(&source[error.labels()[0].span().unwrap()], "*value");
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
        assert_eq!(error.path(), "metadata.channels[0].used_env_vars");
        assert!(error.labels()[0].span().is_some());
        let source = format!("{EMPTY}future_semantics: true\n");
        let error = Document::parse(&source).unwrap_err();
        assert_eq!(error.path(), "future_semantics");
        assert!(error.labels()[0].span().is_some());
    }

    #[test]
    fn duplicate_keys_report_both_definitions() {
        let source = EMPTY.replace(
            "  sources: []",
            "  sources: []\n  custom_metadata: {key: first, key: second}",
        );
        let error = Document::parse(&source).unwrap_err();
        assert_eq!(error.path(), "metadata.custom_metadata.key");
        assert_eq!(error.labels().len(), 2);
        let first = error.labels()[0].span().unwrap();
        let second = error.labels()[1].span().unwrap();
        assert_eq!(&source[first.clone()], "key");
        assert_eq!(&source[second.clone()], "key");
        assert_ne!(first, second);
    }
}
