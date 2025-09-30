use serde::{Deserialize, Serialize};
use std::fmt;

/// Represents a value in the variant configuration.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum VariantValue {
    Bool(bool),
    Integer(i64),
    Float(f64),
    String(String),
}

impl VariantValue {
    /// Convert this value into a `serde_json::Value` so it can be passed to Jinja.
    pub fn to_json(&self) -> serde_json::Value {
        match self {
            VariantValue::Bool(b) => serde_json::Value::Bool(*b),
            VariantValue::Integer(i) => serde_json::Value::Number((*i).into()),
            VariantValue::Float(f) => serde_json::Value::Number(
                serde_json::Number::from_f64(*f).unwrap_or_else(|| serde_json::Number::from(0)),
            ),
            VariantValue::String(s) => serde_json::Value::String(s.clone()),
        }
    }
}

impl fmt::Display for VariantValue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            VariantValue::Bool(b) => write!(f, "{}", b),
            VariantValue::Integer(i) => write!(f, "{}", i),
            VariantValue::Float(fl) => write!(f, "{}", fl),
            VariantValue::String(s) => write!(f, "{}", s),
        }
    }
}

impl From<&str> for VariantValue {
    fn from(value: &str) -> Self {
        VariantValue::String(value.to_string())
    }
}

impl From<String> for VariantValue {
    fn from(value: String) -> Self {
        VariantValue::String(value)
    }
}

impl From<bool> for VariantValue {
    fn from(value: bool) -> Self {
        VariantValue::Bool(value)
    }
}

impl From<i64> for VariantValue {
    fn from(value: i64) -> Self {
        VariantValue::Integer(value)
    }
}

impl From<f64> for VariantValue {
    fn from(value: f64) -> Self {
        VariantValue::Float(value)
    }
}
