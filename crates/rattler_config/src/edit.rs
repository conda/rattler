//! Editing support for [`ConfigBase`]: `set`/`unset` a configuration value
//! by its dotted TOML key path, and save the result back to disk.
//!
//! Editing is implemented generically by round-tripping through TOML: the
//! configuration is serialized to a TOML table, the requested key is
//! updated, and the table is deserialized back. This means it automatically
//! works for every key — including the keys of tool-specific extensions —
//! without any per-field code. Unknown keys are rejected using the same
//! detection that [`ConfigBase::from_toml_str`] uses when loading files.

use std::{path::Path, str::FromStr};

use crate::config::{
    Config, ConfigBase,
    exclude_newer::{ExcludeNewerConfig, ExemptionTable},
};
use rattler_conda_types::PackageName;
use thiserror::Error;
use toml_edit::{InlineTable, Item, TableLike};

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

/// Prepares an edit of a key inside `exclude-newer`, which `set` cannot do on
/// the serialized configuration alone:
/// - A cutoff without exemptions is serialized on its own, e.g.
///   `exclude-newer = "7d"`. It is expanded into its table form, so that
///   e.g. `exclude-newer.exemptions.ruff` keeps the cutoff.
/// - Exemptions are keyed by package name, which is compared normalized. An
///   exemption whose name only differs from the edited one in case is renamed
///   to the edited name, so that e.g. setting `exemptions.Ruff` replaces
///   `ruff` and setting `exemptions.Ruff.build` keeps the other fields of
///   `ruff`.
/// - An exemption that consists of a version spec only is serialized on its
///   own, e.g. `ruff = ">=0.5"`. When a field of it is edited, it is expanded
///   into its table form, so that e.g. `exemptions.ruff.build` keeps the
///   version.
fn prepare_exclude_newer_edit(table: &mut toml::Table, segments: &[String]) {
    let [first, rest @ ..] = segments else {
        return;
    };
    if first != "exclude-newer" || rest.is_empty() {
        return;
    }
    let Some(exclude_newer) = table.get_mut(first) else {
        return;
    };
    if let toml::Value::String(cutoff) = exclude_newer {
        let cutoff = std::mem::take(cutoff);
        *exclude_newer = toml::Value::Table(toml::Table::from_iter([(
            "cutoff".to_string(),
            toml::Value::String(cutoff),
        )]));
    }

    let [exemptions_key, edited, field @ ..] = rest else {
        return;
    };
    if exemptions_key != "exemptions" {
        return;
    }
    let Ok(name) = PackageName::from_str(edited) else {
        return;
    };
    let Some(exemptions) = exclude_newer
        .get_mut("exemptions")
        .and_then(toml::Value::as_table_mut)
    else {
        return;
    };
    let existing = exemptions
        .keys()
        .find(|existing| PackageName::from_str(existing).ok().as_ref() == Some(&name))
        .cloned();
    if let Some(existing) = existing
        && existing != *edited
    {
        let exemption = exemptions
            .remove(&existing)
            .expect("the key was just found");
        exemptions.insert(edited.clone(), exemption);
    }

    if !field.is_empty()
        && let Some(exemption) = exemptions.get_mut(edited)
        && let toml::Value::String(version) = exemption
    {
        let version = std::mem::take(version);
        *exemption = toml::Value::Table(toml::Table::from_iter([(
            "version".to_string(),
            toml::Value::String(version),
        )]));
    }
}

/// Checks that `exclude-newer` is set before an exemption is added to it,
/// which would otherwise fail with a missing field `cutoff`.
fn check_exclude_newer_is_set(
    table: &toml::Table,
    segments: &[String],
    key: &str,
) -> Result<(), ConfigEditError> {
    if let [first, second, ..] = segments
        && first == "exclude-newer"
        && second == "exemptions"
        && !table.contains_key(first)
    {
        return Err(ConfigEditError::InvalidValue {
            key: key.to_string(),
            source: "`exclude-newer` is not set, set its cutoff before adding exemptions, e.g. `exclude-newer = \"7d\"`".into(),
        });
    }
    Ok(())
}

/// Is `error` a type mismatch of a value of type `ty`, e.g. `string`?
fn is_type_error(error: &ConfigEditError, ty: &str) -> bool {
    match error {
        ConfigEditError::InvalidValue { source, .. } => {
            source.to_string().contains(&format!("invalid type: {ty}"))
        }
        _ => false,
    }
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

/// Writes the edit of `key`, a key of `exclude-newer` such as
/// `exclude-newer.exemptions.ruff`, into `document`, the root table of a
/// configuration file, keeping the comments and layout of the rest of it.
///
/// `edited` is `exclude-newer` after the edit, as [`ConfigBase::set`] leaves
/// it, so the edit has been validated and only needs to be written. Only the
/// value of `key` is taken from it, with the same rules as [`ConfigBase::set`]:
/// - A cutoff without exemptions, e.g. `exclude-newer = "7d"`, is expanded
///   into an inline table when a key inside it is edited.
/// - Exemptions are matched by their normalized package name. Unlike
///   [`ConfigBase::set`], an exemption keeps the name as it is written in the
///   document, e.g. setting `exemptions.Ruff` replaces the value of `ruff`.
/// - An exemption that consists of a version spec only, e.g.
///   `ruff = ">=0.5"`, is expanded into an inline table when a field of it is
///   edited.
/// - Without exemptions, `exclude-newer` is written as its cutoff alone, so
///   removing the last exemption leaves e.g. `exclude-newer = "7d"`.
///
/// Fails with [`ConfigEditError::UnknownKey`] if `key` is not `exclude-newer`
/// or a key inside it.
pub fn write_exclude_newer_edit(
    document: &mut toml_edit::Table,
    key: &str,
    edited: Option<&ExcludeNewerConfig>,
) -> Result<(), ConfigEditError> {
    const EXCLUDE_NEWER: &str = "exclude-newer";
    let segments = split_key(key);
    let [first, rest @ ..] = segments.as_slice() else {
        unreachable!("a key has at least one segment")
    };
    if first != EXCLUDE_NEWER || segments.iter().any(String::is_empty) {
        return Err(ConfigEditError::UnknownKey {
            key: key.to_string(),
            supported_keys: EXCLUDE_NEWER.to_string(),
        });
    }
    let Some(edited) = edited else {
        document.remove(EXCLUDE_NEWER);
        return Ok(());
    };

    let edited_in_place = !rest.is_empty()
        && document
            .get_mut(EXCLUDE_NEWER)
            .and_then(|item| write_edited_value(item, rest, edited).transpose())
            .transpose()?
            .is_some();
    if !edited_in_place {
        // The whole value is replaced, e.g. when it isn't set yet.
        let value = to_edit_value(&toml::Value::try_from(edited)?);
        set_value(document, EXCLUDE_NEWER, value);
    }
    Ok(())
}

/// Writes the value of the key `rest` inside `exclude_newer` from `edited`.
/// Returns `None` if `exclude_newer` doesn't have the shape the key needs.
fn write_edited_value(
    exclude_newer: &mut Item,
    rest: &[String],
    edited: &ExcludeNewerConfig,
) -> Result<Option<()>, ConfigEditError> {
    let new_value = edited_value(edited, rest)?;
    expand_string(exclude_newer, "cutoff");
    let mut inline = exclude_newer.is_inline_table();
    let Some(table) = exclude_newer.as_table_like_mut() else {
        return Ok(None);
    };

    let (leaf, parents) = rest
        .split_last()
        .expect("the key is inside `exclude-newer`");
    let mut current: &mut dyn TableLike = table;
    for (depth, segment) in parents.iter().enumerate() {
        let spelling = existing_spelling(current, rest, depth).unwrap_or_else(|| segment.clone());
        let child = current
            .entry(&spelling)
            .or_insert_with(|| Item::Value(InlineTable::new().into()));
        if is_exemption(rest, depth) {
            expand_string(child, "version");
        }
        inline |= child.is_inline_table();
        let Some(child) = child.as_table_like_mut() else {
            return Ok(None);
        };
        current = child;
    }
    let spelling = existing_spelling(current, rest, parents.len()).unwrap_or_else(|| leaf.clone());
    match new_value {
        Some(value) => set_value(current, &spelling, to_edit_value(&value)),
        None => remove_key(current, &spelling, inline),
    }

    // Without exemptions, the cutoff is written on its own.
    let table = exclude_newer
        .as_table_like_mut()
        .expect("the type of `exclude-newer` is unchanged");
    if table
        .get("exemptions")
        .and_then(Item::as_table_like)
        .is_some_and(TableLike::is_empty)
    {
        table.remove("exemptions");
    }
    if edited.exemptions.is_empty() && table.len() == 1 {
        let Item::Value(value) = exclude_newer else {
            // A table header is replaced by the whole value.
            return Ok(None);
        };
        let decor = value.decor().clone();
        *value = edited.cutoff.as_str().into();
        *value.decor_mut() = decor;
    }
    Ok(Some(()))
}

/// The value of the key `rest` inside `exclude-newer` after the edit, or
/// `None` if it is not set.
fn edited_value(
    edited: &ExcludeNewerConfig,
    rest: &[String],
) -> Result<Option<toml::Value>, ConfigEditError> {
    Ok(match rest {
        [cutoff] if cutoff == "cutoff" => Some(toml::Value::String(edited.cutoff.to_string())),
        [exemptions] if exemptions == "exemptions" => (!edited.exemptions.is_empty())
            .then(|| toml::Value::try_from(&edited.exemptions))
            .transpose()?,
        [exemptions, name, field @ ..] if exemptions == "exemptions" => {
            let Some(exemption) = PackageName::from_str(name)
                .ok()
                .and_then(|name| edited.exemptions.get(&name))
            else {
                return Ok(None);
            };
            if field.is_empty() {
                Some(toml::Value::try_from(exemption)?)
            } else {
                // A field is looked up in the table form, as an exemption
                // that is a version spec only is written on its own.
                let mut value = toml::Value::try_from(ExemptionTable::from(exemption))?;
                for segment in field {
                    match value {
                        toml::Value::Table(mut table) => match table.remove(segment) {
                            Some(next) => value = next,
                            None => return Ok(None),
                        },
                        _ => return Ok(None),
                    }
                }
                Some(value)
            }
        }
        _ => None,
    })
}

/// Is the segment of `rest` at `depth` the name of an exemption?
fn is_exemption(rest: &[String], depth: usize) -> bool {
    depth == 1 && rest[0] == "exemptions"
}

/// The key of `table` that the segment of `rest` at `depth` addresses, as it
/// is written. Exemptions are matched by their normalized package name.
fn existing_spelling(table: &dyn TableLike, rest: &[String], depth: usize) -> Option<String> {
    let segment = &rest[depth];
    if table.contains_key(segment) {
        return Some(segment.clone());
    }
    if !is_exemption(rest, depth) {
        return None;
    }
    let name = PackageName::from_str(segment).ok()?;
    table
        .iter()
        .map(|(existing, _)| existing)
        .find(|existing| PackageName::from_str(existing).ok().as_ref() == Some(&name))
        .map(str::to_string)
}

/// Expands a string value into an inline table that holds it as `key`, e.g.
/// the cutoff `"7d"` into `{ cutoff = "7d" }`, keeping its decor.
fn expand_string(item: &mut Item, key: &str) {
    if let Item::Value(value @ toml_edit::Value::String(_)) = item {
        let decor = value.decor().clone();
        let mut string = std::mem::replace(value, InlineTable::new().into());
        string.decor_mut().clear();
        let table = value.as_inline_table_mut().expect("just replaced");
        table.insert(key, string);
        *value.decor_mut() = decor;
    }
}

/// Sets `key` of `table` to `value`, keeping the comments around a value it
/// replaces. A table it replaces is removed, as its key is written as a table
/// header.
fn set_value(table: &mut dyn TableLike, key: &str, mut value: toml_edit::Value) {
    match table.get_mut(key) {
        Some(Item::Value(existing)) => {
            *value.decor_mut() = existing.decor().clone();
            *existing = value;
        }
        Some(_) => {
            table.remove(key);
            table.insert(key, Item::Value(value));
        }
        None => {
            table.insert(key, Item::Value(value));
        }
    }
}

/// Removes `key` from `table`. In an inline table, the value before the last
/// one takes over its trailing whitespace, e.g. `{ a = "1", b = "2" }` becomes
/// `{ a = "1" }` rather than `{ a = "1"}`.
fn remove_key(table: &mut dyn TableLike, key: &str, inline: bool) {
    let keys: Vec<String> = table.iter().map(|(k, _)| k.to_string()).collect();
    let previous = match keys.as_slice() {
        [.., previous, last] if last == key => Some(previous.clone()),
        _ => None,
    };
    let Some(Item::Value(removed)) = table.remove(key) else {
        return;
    };
    if inline
        && let Some(previous) = previous
        && let Some(Item::Value(previous)) = table.get_mut(&previous)
    {
        let suffix = removed
            .decor()
            .suffix()
            .cloned()
            .unwrap_or_else(|| " ".into());
        previous.decor_mut().set_suffix(suffix);
    }
}

/// Converts a TOML value into a value of a `toml_edit` document.
fn to_edit_value(value: &toml::Value) -> toml_edit::Value {
    value
        .to_string()
        .parse()
        .expect("a serialized TOML value parses")
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
        prepare_exclude_newer_edit(&mut table, &segments);

        if let Some(raw) = value {
            check_exclude_newer_is_set(&table, &segments, key)?;
            let parsed = parse_value(&raw);
            let retry_as_string = !matches!(parsed, toml::Value::String(_));
            insert_path(&mut table, &segments, parsed);

            match self.rebuild_from(&table, key) {
                Ok(()) => Ok(()),
                Err(first_error) if retry_as_string => {
                    // The JSON interpretation did not fit the target
                    // field (e.g. `region = "true"`); retry verbatim. If the
                    // type of the JSON value is wrong but the field takes a
                    // string, the error of the verbatim value is the
                    // relevant one (e.g. an invalid cutoff `2026`).
                    insert_path(&mut table, &segments, toml::Value::String(raw));
                    self.rebuild_from(&table, key).map_err(|retry_error| {
                        if is_type_error(&first_error, "") && !is_type_error(&retry_error, "string")
                        {
                            retry_error
                        } else {
                            first_error
                        }
                    })
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
    use crate::config::NoExtension;

    /// Edits `key` of the configuration file `document` with
    /// [`ConfigBase::set`] and writes it with [`write_exclude_newer_edit`].
    fn edit_document(document: &str, key: &str, value: Option<&str>) -> String {
        let (mut config, _) = ConfigBase::<NoExtension>::from_toml_str(document).unwrap();
        config.set(key, value.map(str::to_string)).unwrap();
        let mut document: toml_edit::DocumentMut = document.parse().unwrap();
        write_exclude_newer_edit(
            document.as_table_mut(),
            key,
            config.common.exclude_newer.as_ref(),
        )
        .unwrap();
        let written = document.to_string();
        let (reloaded, _) = ConfigBase::<NoExtension>::from_toml_str(&written).unwrap();
        assert_eq!(reloaded.common.exclude_newer, config.common.exclude_newer);
        written
    }

    #[test]
    fn write_exclude_newer_edit_keeps_comments_and_layout() {
        let document = r#"# Packages
default-channels = ["conda-forge"]

[exclude-newer]
cutoff = "7d" # a week
[exclude-newer.exemptions]
# keep ruff current
ruff = "*"
numpy = ">=2" # pinned

[concurrency]
solves = 2
"#;
        let document = edit_document(document, "exclude-newer.cutoff", Some("14d"));
        let document = edit_document(
            &document,
            "exclude-newer.exemptions.numpy.build",
            Some("py3*"),
        );
        let document = edit_document(&document, "exclude-newer.exemptions.Polars", Some("==1"));
        let document = edit_document(&document, "exclude-newer.exemptions.RUFF", Some(">=0.5"));
        assert_eq!(
            document,
            r#"# Packages
default-channels = ["conda-forge"]

[exclude-newer]
cutoff = "14d" # a week
[exclude-newer.exemptions]
# keep ruff current
ruff = ">=0.5"
numpy = { version = ">=2", build = "py3*" } # pinned
Polars = "==1"

[concurrency]
solves = 2
"#
        );

        let document = edit_document(&document, "exclude-newer.exemptions.numpy.build", None);
        let document = edit_document(&document, "exclude-newer.exemptions.polars", None);
        assert_eq!(
            document,
            r#"# Packages
default-channels = ["conda-forge"]

[exclude-newer]
cutoff = "14d" # a week
[exclude-newer.exemptions]
# keep ruff current
ruff = ">=0.5"
numpy = { version = ">=2" } # pinned

[concurrency]
solves = 2
"#
        );
    }

    #[test]
    fn write_exclude_newer_edit_of_a_bare_cutoff() {
        let document = "exclude-newer = \"7d\" # a week\n";
        let document = edit_document(document, "exclude-newer.exemptions.ruff", Some("*"));
        assert_eq!(
            document,
            "exclude-newer = { cutoff = \"7d\", exemptions = { ruff = \"*\" } } # a week\n"
        );
        let document = edit_document(
            &document,
            "exclude-newer.exemptions.ruff.build",
            Some("py*"),
        );
        assert_eq!(
            document,
            "exclude-newer = { cutoff = \"7d\", exemptions = { ruff = { version = \"*\", build = \"py*\" } } } # a week\n"
        );

        // Without exemptions, the cutoff is written on its own.
        let document = edit_document(&document, "exclude-newer.exemptions.Ruff", None);
        assert_eq!(document, "exclude-newer = \"7d\" # a week\n");
        let document = edit_document(&document, "exclude-newer.cutoff", Some("1d"));
        assert_eq!(document, "exclude-newer = \"1d\" # a week\n");
    }

    #[test]
    fn write_exclude_newer_edit_collapses_a_table_without_exemptions() {
        let document =
            "[exclude-newer]\ncutoff = \"7d\"\n\n[exclude-newer.exemptions]\nruff = \"*\"\n";
        let document = edit_document(document, "exclude-newer.exemptions.ruff", None);
        assert_eq!(document, "exclude-newer = \"7d\"\n");
    }

    #[test]
    fn write_exclude_newer_edit_of_the_whole_value() {
        let document = "offline = true\n";
        let document = edit_document(document, "exclude-newer", Some("7d"));
        assert_eq!(document, "offline = true\nexclude-newer = \"7d\"\n");
        let document = edit_document(&document, "exclude-newer.cutoff", Some("14d"));
        assert_eq!(document, "offline = true\nexclude-newer = \"14d\"\n");
        let document = edit_document(&document, "exclude-newer", None);
        assert_eq!(document, "offline = true\n");
    }

    #[test]
    fn write_exclude_newer_edit_rejects_other_keys() {
        let mut document = toml_edit::DocumentMut::new();
        assert!(matches!(
            write_exclude_newer_edit(document.as_table_mut(), "offline", None),
            Err(ConfigEditError::UnknownKey { .. })
        ));
        assert!(matches!(
            write_exclude_newer_edit(document.as_table_mut(), "exclude-newer..cutoff", None),
            Err(ConfigEditError::UnknownKey { .. })
        ));
    }

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
