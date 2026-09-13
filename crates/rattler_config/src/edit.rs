//! Editing support for [`ConfigBase`]: `set`/`unset` a configuration value
//! by its dotted TOML key path, and save the result back to disk.
//!
//! Editing is implemented generically by round-tripping through TOML: the
//! configuration is serialized to a TOML table, the requested key is
//! updated, and the table is deserialized back. This means it automatically
//! works for every key — including the keys of tool-specific extensions —
//! without any per-field code. Unknown keys are rejected using the same
//! detection that [`ConfigBase::from_toml_str`] uses when loading files.

use std::path::Path;

use crate::config::{Config, ConfigBase};
use thiserror::Error;

#[derive(Error, Debug)]
pub enum ConfigEditError {
    #[error("Unknown configuration key: {key}\nSupported keys:\n\t{supported_keys}")]
    UnknownKey { key: String, supported_keys: String },

    #[error("Invalid value for '{key}': {source}")]
    InvalidValue {
        key: String,
        #[source]
        source: Box<dyn std::error::Error + Send + Sync>,
    },

    #[error("IO error: {0}")]
    IoError(#[from] std::io::Error),

    #[error("TOML serialization error: {0}")]
    TomlSerializeError(#[from] toml::ser::Error),
}

/// Split a dotted key path into segments, honoring TOML-style quoting so
/// that keys containing dots (URLs, bucket names) can be addressed:
/// `mirrors."https://conda.anaconda.org"`.
fn split_key(key: &str) -> Vec<String> {
    let mut segments = Vec::new();
    let mut current = String::new();
    let mut quote: Option<char> = None;

    for c in key.chars() {
        match (c, quote) {
            (q, Some(open)) if q == open => quote = None,
            ('"' | '\'', None) => quote = Some(c),
            ('.', None) => {
                segments.push(std::mem::take(&mut current));
                current.clear();
            }
            _ => current.push(c),
        }
    }
    segments.push(current);
    segments
}

/// Convert a JSON value into the equivalent TOML value. JSON `null` has no
/// TOML equivalent and is mapped back to the literal string `"null"`.
fn json_to_toml(value: serde_json::Value) -> toml::Value {
    match value {
        serde_json::Value::Null => toml::Value::String("null".to_string()),
        serde_json::Value::Bool(b) => toml::Value::Boolean(b),
        serde_json::Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                toml::Value::Integer(i)
            } else {
                toml::Value::Float(n.as_f64().unwrap_or(f64::NAN))
            }
        }
        serde_json::Value::String(s) => toml::Value::String(s),
        serde_json::Value::Array(values) => {
            toml::Value::Array(values.into_iter().map(json_to_toml).collect())
        }
        serde_json::Value::Object(map) => {
            toml::Value::Table(map.into_iter().map(|(k, v)| (k, json_to_toml(v))).collect())
        }
    }
}

/// Parse a value passed on the command line. Values are interpreted as JSON
/// when possible (`true`, `5`, `["conda-forge"]`,
/// `{"endpoint-url": "…"}`) and fall back to a plain string otherwise.
fn parse_value(raw: &str) -> toml::Value {
    match serde_json::from_str::<serde_json::Value>(raw) {
        Ok(json) => json_to_toml(json),
        Err(_) => toml::Value::String(raw.to_string()),
    }
}

/// Insert `value` at the given path, creating intermediate tables as needed.
fn insert_path(table: &mut toml::Table, segments: &[String], value: toml::Value) {
    let (last, intermediate) = segments
        .split_last()
        .expect("path has at least one segment");
    let mut current = table;
    for segment in intermediate {
        let entry = current
            .entry(segment.clone())
            .or_insert_with(|| toml::Value::Table(toml::Table::new()));
        if !entry.is_table() {
            *entry = toml::Value::Table(toml::Table::new());
        }
        current = entry.as_table_mut().expect("just ensured this is a table");
    }
    current.insert(last.clone(), value);
}

/// Remove the value at the given path, if present. Returns whether a value
/// was removed.
fn remove_path(table: &mut toml::Table, segments: &[String]) -> bool {
    let (last, intermediate) = segments
        .split_last()
        .expect("path has at least one segment");
    let mut current = table;
    for segment in intermediate {
        match current.get_mut(segment).and_then(toml::Value::as_table_mut) {
            Some(next) => current = next,
            None => return false,
        }
    }
    current.remove(last).is_some()
}

/// Does `key` address (part of) one of the `known` dotted key paths?
fn is_known_key(key: &str, known: &[String]) -> bool {
    known.iter().any(|k| {
        k == key
            || k.strip_prefix(key)
                .is_some_and(|rest| rest.starts_with('.'))
            || key
                .strip_prefix(k.as_str())
                .is_some_and(|rest| rest.starts_with('.'))
    })
}

impl<T> ConfigBase<T>
where
    T: Config,
{
    /// Modify this config with the given key and value.
    ///
    /// - `Some(value)`: set the key. The value is interpreted as JSON when
    ///   possible and as a plain string otherwise.
    /// - `None`: unset the key, resetting it to its default.
    ///
    /// Keys are dotted TOML paths (`concurrency.solves`,
    /// `s3-options.my-bucket.region`); segments containing dots can be
    /// quoted (`mirrors."https://conda.anaconda.org"`). Setting a key that
    /// neither the common configuration nor the extension understands
    /// results in [`ConfigEditError::UnknownKey`].
    ///
    /// It is required to call `save()` to persist the changes on disk.
    pub fn set(&mut self, key: &str, value: Option<String>) -> Result<(), ConfigEditError> {
        let segments = split_key(key);
        let unknown_key = |config: &Self| ConfigEditError::UnknownKey {
            key: key.to_string(),
            supported_keys: config.keys().join(",\n\t"),
        };
        if segments.iter().any(String::is_empty) {
            return Err(unknown_key(self));
        }

        let mut table = toml::Table::try_from(&*self)?;

        if let Some(raw) = value {
            let parsed = parse_value(&raw);
            let retry_as_string = !matches!(parsed, toml::Value::String(_));
            insert_path(&mut table, &segments, parsed);

            match self.rebuild_from(&table, key) {
                Ok(()) => Ok(()),
                Err(first_error) if retry_as_string => {
                    // The JSON interpretation did not fit the target
                    // field (e.g. `region = "true"`); retry verbatim.
                    insert_path(&mut table, &segments, toml::Value::String(raw));
                    self.rebuild_from(&table, key)
                        .map_err(|_retry_error| first_error)
                }
                Err(err) => Err(err),
            }
        } else {
            // Removing a key that is not set is fine (unsetting an
            // already-unset option), but the key itself must be known.
            if !is_known_key(key, &self.keys()) {
                return Err(unknown_key(self));
            }
            remove_path(&mut table, &segments);
            self.rebuild_from(&table, key)
        }
    }

    /// Deserialize `table` back into `self`, preserving the fields that are
    /// not part of the serialized representation (`channel_config`,
    /// `loaded_from`). Fails if the table contains keys that neither the
    /// common configuration nor the extension recognizes.
    fn rebuild_from(&mut self, table: &toml::Table, key: &str) -> Result<(), ConfigEditError> {
        let rendered = toml::to_string(table)?;
        let (mut new_config, unused) =
            Self::from_toml_str(&rendered).map_err(|e| ConfigEditError::InvalidValue {
                key: key.to_string(),
                source: Box::new(e),
            })?;

        if !unused.is_empty() {
            return Err(ConfigEditError::UnknownKey {
                key: key.to_string(),
                supported_keys: self.keys().join(",\n\t"),
            });
        }

        new_config.loaded_from = std::mem::take(&mut self.loaded_from);
        new_config.common.channel_config = self.common.channel_config.clone();
        *self = new_config;
        Ok(())
    }

    /// Save the config to the given path.
    pub fn save(&self, to: &Path) -> Result<(), ConfigEditError> {
        let contents = self.to_toml()?;
        tracing::debug!("Saving config to: {}", to.display());

        let parent = to.parent().expect("config path should have a parent");
        fs_err::create_dir_all(parent)?;

        fs_err::write(to, contents)?;
        Ok(())
    }

    /// Convert the config to a TOML string.
    pub fn to_toml(&self) -> Result<String, ConfigEditError> {
        toml::to_string_pretty(&self).map_err(ConfigEditError::TomlSerializeError)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn split_key_plain() {
        assert_eq!(
            split_key("concurrency.solves"),
            vec!["concurrency", "solves"]
        );
    }

    #[test]
    fn split_key_quoted_segment_keeps_dots() {
        assert_eq!(
            split_key(r#"mirrors."https://conda.anaconda.org""#),
            vec!["mirrors", "https://conda.anaconda.org"]
        );
    }

    #[test]
    fn parse_value_json_and_fallback() {
        assert_eq!(parse_value("true"), toml::Value::Boolean(true));
        assert_eq!(parse_value("5"), toml::Value::Integer(5));
        assert_eq!(
            parse_value("plain string"),
            toml::Value::String("plain string".to_string())
        );
        assert!(matches!(parse_value("[1, 2]"), toml::Value::Array(_)));
        assert!(matches!(parse_value(r#"{"a": 1}"#), toml::Value::Table(_)));
    }
}
