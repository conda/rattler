use crate::{context::VariantContext, normalized_key::NormalizedKey, variant_value::VariantValue};
use fs_err as fs;
use miette::Diagnostic;
use minijinja::Environment;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use thiserror::Error;

pub const CONDA_BUILD_CONFIG_FILE: &str = "conda_build_config.yaml";

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
/// Represents a pin configuration for a package.
pub struct Pin {
    /// The maximum pin (a string like "x.x.x").
    pub max_pin: Option<String>,
    /// The minimum pin (a string like "x.x.x").
    pub min_pin: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub struct VariantConfig {
    pin_run_as_build: Option<BTreeMap<String, Pin>>,
    zip_keys: Option<Vec<Vec<NormalizedKey>>>,
    variants: BTreeMap<NormalizedKey, Vec<VariantValue>>,
}

impl VariantConfig {
    pub fn from_files(
        files: &[PathBuf],
        context: &VariantContext,
    ) -> Result<Self, VariantConfigError> {
        let mut final_config = VariantConfig::default();
        for filename in files {
            let config = if filename.file_name() == Some(CONDA_BUILD_CONFIG_FILE.as_ref()) {
                load_conda_build_config(filename, context)?
            } else {
                load_variant_file(filename, context)?
            };
            final_config.merge(config);
        }

        final_config.insert_platforms(context);
        Ok(final_config)
    }

    fn merge(&mut self, other: VariantConfig) {
        self.variants.extend(other.variants);
        match (&mut self.pin_run_as_build, other.pin_run_as_build) {
            (Some(existing), Some(new)) => existing.extend(new),
            (None, Some(new)) => self.pin_run_as_build = Some(new),
            _ => {}
        }
        if other.zip_keys.is_some() {
            self.zip_keys = other.zip_keys;
        }
    }

    fn insert_platforms(&mut self, context: &VariantContext) {
        self.variants.insert(
            NormalizedKey::from("target_platform"),
            vec![VariantValue::from(context.target_platform.to_string())],
        );
        self.variants.insert(
            NormalizedKey::from("build_platform"),
            vec![VariantValue::from(context.build_platform.to_string())],
        );
    }

    pub fn pin_run_as_build(&self) -> Option<&BTreeMap<String, Pin>> {
        self.pin_run_as_build.as_ref()
    }

    pub fn zip_keys(&self) -> Option<&Vec<Vec<NormalizedKey>>> {
        self.zip_keys.as_ref()
    }

    pub fn variants(&self) -> &BTreeMap<NormalizedKey, Vec<VariantValue>> {
        &self.variants
    }
}

#[derive(Debug, Error, Diagnostic)]
pub enum VariantConfigError {
    #[error("Could not open file ({0}): {1}")]
    Io(PathBuf, #[source] std::io::Error),
    #[error("Could not parse variant config file ({0}): {1}")]
    Parse(PathBuf, #[source] serde_yaml::Error),
    #[error("Invalid variant config structure in {0}: {1}")]
    InvalidStructure(PathBuf, String),
    #[error("Failed to evaluate selector '{1}' in {0}: {2}")]
    Selector(PathBuf, String, String),
    #[error("Failed to evaluate template in {0}: {1}")]
    Template(PathBuf, String),
}

fn load_variant_file(
    path: &Path,
    context: &VariantContext,
) -> Result<VariantConfig, VariantConfigError> {
    let contents =
        fs::read_to_string(path).map_err(|err| VariantConfigError::Io(path.to_path_buf(), err))?;

    let mut env = Environment::new();
    env.set_trim_blocks(true);
    env.set_lstrip_blocks(true);
    env.add_function("environ_get", |name: String, default: Option<String>| {
        let value = std::env::var(&name).unwrap_or_else(|_| default.unwrap_or_default());
        Ok(minijinja::value::Value::from(value))
    });

    let ctx = context.as_json_context();
    let yaml_value: serde_yaml::Value = serde_yaml::from_str(&contents)
        .map_err(|err| VariantConfigError::Parse(path.to_path_buf(), err))?;
    let flattened = flatten_selectors(yaml_value, path, &env, &ctx)?;
    build_variant_config(path, flattened)
}

fn flatten_selectors(
    value: serde_yaml::Value,
    path: &Path,
    env: &Environment<'_>,
    ctx: &serde_json::Value,
) -> Result<serde_yaml::Value, VariantConfigError> {
    match value {
        serde_yaml::Value::Mapping(map) => {
            let mut result = serde_yaml::Mapping::new();
            for (key, value) in map {
                let key_str = match key {
                    serde_yaml::Value::String(s) => s,
                    other => {
                        return Err(VariantConfigError::InvalidStructure(
                            path.to_path_buf(),
                            format!("expected string key but found {other:?}"),
                        ))
                    }
                };

                if let Some(condition) = key_str
                    .strip_prefix("sel(")
                    .and_then(|s| s.strip_suffix(')'))
                {
                    if evaluate_selector(condition, path, env, ctx)? {
                        let nested = flatten_selectors(value, path, env, ctx)?;
                        merge_mapping(path, &mut result, nested)?;
                    }
                    continue;
                }

                let rendered_key = render_string(path, env, ctx, &key_str)?;
                let flattened_value = flatten_selectors(value, path, env, ctx)?;
                result.insert(serde_yaml::Value::String(rendered_key), flattened_value);
            }
            Ok(serde_yaml::Value::Mapping(result))
        }
        serde_yaml::Value::Sequence(seq) => {
            let mut result = Vec::new();
            for item in seq {
                let flattened_item = flatten_selectors(item, path, env, ctx)?;
                if !flattened_item.is_null() {
                    result.push(flattened_item);
                }
            }
            Ok(serde_yaml::Value::Sequence(result))
        }
        serde_yaml::Value::String(s) => {
            let rendered = render_string(path, env, ctx, &s)?;
            Ok(serde_yaml::Value::String(rendered))
        }
        other => Ok(other),
    }
}

fn merge_mapping(
    path: &Path,
    dest: &mut serde_yaml::Mapping,
    value: serde_yaml::Value,
) -> Result<(), VariantConfigError> {
    match value {
        serde_yaml::Value::Mapping(map) => {
            for (key, value) in map {
                dest.insert(key, value);
            }
            Ok(())
        }
        serde_yaml::Value::Null => Ok(()),
        other => Err(VariantConfigError::InvalidStructure(
            path.to_path_buf(),
            format!("selector must evaluate to a mapping, found {other:?}"),
        )),
    }
}

fn render_string(
    path: &Path,
    env: &Environment<'_>,
    ctx: &serde_json::Value,
    input: &str,
) -> Result<String, VariantConfigError> {
    let template = preprocess_template(input);
    env.render_str(&template, ctx)
        .map_err(|err| VariantConfigError::Template(path.to_path_buf(), err.to_string()))
        .map(|s| s.trim().to_string())
}

fn preprocess_template(input: &str) -> String {
    if input.contains("${{") {
        input.replace("${{", "{{").replace("}}", "}}")
    } else {
        input.to_string()
    }
}

fn evaluate_selector(
    condition: &str,
    path: &Path,
    env: &Environment<'_>,
    ctx: &serde_json::Value,
) -> Result<bool, VariantConfigError> {
    let template = format!("{{% if {condition} %}}true{{% else %}}false{{% endif %}}");
    let rendered = env.render_str(&template, ctx).map_err(|err| {
        VariantConfigError::Selector(path.to_path_buf(), condition.to_string(), err.to_string())
    })?;
    Ok(rendered.trim() == "true")
}

fn build_variant_config(
    path: &Path,
    value: serde_yaml::Value,
) -> Result<VariantConfig, VariantConfigError> {
    if value.is_null() {
        return Ok(VariantConfig::default());
    }

    let raw: RawVariantFile = serde_yaml::from_value(value)
        .map_err(|err| VariantConfigError::Parse(path.to_path_buf(), err))?;

    let mut variants = BTreeMap::new();
    for (key, entry) in raw.entries {
        match entry {
            serde_yaml::Value::Sequence(items) => {
                let mut values = Vec::new();
                for item in items {
                    values.push(convert_scalar(path, &key, item)?);
                }
                variants.insert(NormalizedKey::from(key), values);
            }
            serde_yaml::Value::Null => {}
            serde_yaml::Value::Bool(_)
            | serde_yaml::Value::Number(_)
            | serde_yaml::Value::String(_) => {
                let value = convert_scalar(path, &key, entry)?;
                variants.insert(NormalizedKey::from(key), vec![value]);
            }
            other => {
                return Err(VariantConfigError::InvalidStructure(
                    path.to_path_buf(),
                    format!("expected a list of values for key, found {other:?}"),
                ));
            }
        }
    }

    let zip_keys = raw.zip_keys.map(|keys| {
        keys.into_iter()
            .map(|inner| inner.into_iter().map(NormalizedKey::from).collect())
            .collect()
    });

    Ok(VariantConfig {
        pin_run_as_build: raw.pin_run_as_build,
        zip_keys,
        variants,
    })
}

#[derive(Debug, Deserialize)]
struct RawVariantFile {
    #[serde(default)]
    pin_run_as_build: Option<BTreeMap<String, Pin>>,
    #[serde(default)]
    zip_keys: Option<Vec<Vec<String>>>,
    #[serde(flatten)]
    entries: BTreeMap<String, serde_yaml::Value>,
}

fn convert_scalar(
    path: &Path,
    key: &str,
    value: serde_yaml::Value,
) -> Result<VariantValue, VariantConfigError> {
    match value {
        serde_yaml::Value::Bool(b) => Ok(VariantValue::Bool(b)),
        serde_yaml::Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                Ok(VariantValue::Integer(i))
            } else if let Some(f) = n.as_f64() {
                Ok(VariantValue::Float(f))
            } else {
                Err(VariantConfigError::InvalidStructure(
                    path.to_path_buf(),
                    format!("unsupported numeric value for key {key}"),
                ))
            }
        }
        serde_yaml::Value::String(s) => Ok(VariantValue::String(s)),
        other => Err(VariantConfigError::InvalidStructure(
            path.to_path_buf(),
            format!("expected scalar value for key {key}, found {other:?}"),
        )),
    }
}

fn load_conda_build_config(
    path: &Path,
    context: &VariantContext,
) -> Result<VariantConfig, VariantConfigError> {
    let mut input =
        fs::read_to_string(path).map_err(|err| VariantConfigError::Io(path.to_path_buf(), err))?;

    let mut env = Environment::new();
    env.add_function("environ_get", |name: String, default: Option<String>| {
        let value = std::env::var(&name).unwrap_or_else(|_| default.unwrap_or_default());
        Ok(minijinja::value::Value::from(value))
    });

    input = input.replace("os.environ.get", "environ_get");
    input = input.replace(".startswith", " is startingwith");

    let ctx = context.as_json_context();
    let mut lines = Vec::new();
    for line in input.lines() {
        let parsed = ParsedLine::from_str(line);
        if let Some(condition) = parsed.condition {
            if !evaluate_selector(condition, path, &env, &ctx)? {
                continue;
            }
        }
        lines.push(parsed.content.to_string());
    }

    let out = lines.join(
        "
",
    );
    let yaml_value: serde_yaml::Value = serde_yaml::from_str(&out)
        .map_err(|err| VariantConfigError::Parse(path.to_path_buf(), err))?;

    let filtered = match yaml_value {
        serde_yaml::Value::Mapping(map) => {
            let filtered = map
                .into_iter()
                .filter(|(_, v)| !v.is_null())
                .collect::<serde_yaml::Mapping>();
            serde_yaml::Value::Mapping(filtered)
        }
        other => other,
    };

    build_variant_config(path, filtered)
}

#[derive(Debug)]
struct ParsedLine<'a> {
    content: &'a str,
    condition: Option<&'a str>,
}

impl<'a> ParsedLine<'a> {
    fn from_str(line: &'a str) -> ParsedLine<'a> {
        match line.split_once('#') {
            Some((content, cond)) => ParsedLine {
                content: content.trim_end(),
                condition: cond
                    .trim()
                    .strip_prefix('[')
                    .and_then(|s| s.strip_suffix(']'))
                    .map(str::trim),
            },
            None => ParsedLine {
                content: line.trim_end(),
                condition: None,
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;
    use rattler_conda_types::Platform;

    fn context() -> VariantContext {
        VariantContext::new(Platform::Linux64, Platform::Linux64, Platform::Linux64)
    }

    #[test]
    fn applies_selector_condition() {
        let yaml = r#"
sel(linux):
  python:
    - "3.10"
sel(win):
  python:
    - "3.9"
"#;
        let path = Path::new("sel.yaml");
        let env = Environment::new();
        let ctx = context().as_json_context();
        let value: serde_yaml::Value = serde_yaml::from_str(yaml).unwrap();
        let flattened = flatten_selectors(value, path, &env, &ctx).unwrap();
        let config = build_variant_config(path, flattened).unwrap();
        let python = config
            .variants()
            .get(&NormalizedKey::from("python"))
            .unwrap();
        assert_eq!(python, &vec![VariantValue::String("3.10".into())]);
    }

    #[test]
    fn parses_basic_variant_file() {
        let yaml = r#"
python:
  - "3.10"
  - "3.11"
zip_keys:
  - [python, numpy]
numpy:
  - "1.26"
  - "1.26"
"#;
        let path = Path::new("test.yaml");
        let env = Environment::new();
        let ctx = context().as_json_context();
        let value: serde_yaml::Value = serde_yaml::from_str(yaml).unwrap();
        let flattened = flatten_selectors(value, path, &env, &ctx).unwrap();
        let config = build_variant_config(path, flattened).unwrap();
        assert_eq!(config.variants().len(), 2);
        assert_eq!(config.zip_keys().unwrap().len(), 1);
    }
}
