use crate::{normalized_key::NormalizedKey, variant_value::VariantValue};
use rattler_conda_types::Platform;
use serde::Serialize;
use std::collections::BTreeMap;
use strum::IntoEnumIterator as _;

/// Context information required when rendering variant configuration files.
#[derive(Debug, Clone, Serialize)]
pub struct VariantContext {
    pub target_platform: Platform,
    pub host_platform: Platform,
    pub build_platform: Platform,
    #[serde(skip)]
    pub variant: BTreeMap<NormalizedKey, VariantValue>,
}

impl VariantContext {
    pub fn new(
        target_platform: Platform,
        host_platform: Platform,
        build_platform: Platform,
    ) -> Self {
        Self {
            target_platform,
            host_platform,
            build_platform,
            variant: BTreeMap::new(),
        }
    }

    pub fn with_variant(mut self, variant: BTreeMap<NormalizedKey, VariantValue>) -> Self {
        self.variant = variant;
        self
    }

    pub fn variant(&self) -> &BTreeMap<NormalizedKey, VariantValue> {
        &self.variant
    }

    pub(crate) fn as_json_context(&self) -> serde_json::Value {
        use serde_json::json;

        let mut ctx = serde_json::Map::new();
        ctx.insert(
            "target_platform".to_string(),
            json!(self.target_platform.to_string()),
        );
        ctx.insert(
            "host_platform".to_string(),
            json!(self.host_platform.to_string()),
        );
        ctx.insert(
            "build_platform".to_string(),
            json!(self.build_platform.to_string()),
        );
        ctx.insert(
            "unix".to_string(),
            serde_json::Value::Bool(self.host_platform.is_unix()),
        );

        for platform in Platform::iter() {
            if let Some(only_platform) = platform.only_platform() {
                let key = only_platform.to_string();
                ctx.insert(
                    key,
                    serde_json::Value::Bool(
                        self.host_platform.only_platform() == Some(only_platform),
                    ),
                );
            }

            if let Some(arch) = platform.arch() {
                let key = arch.to_string();
                ctx.insert(
                    key,
                    serde_json::Value::Bool(self.host_platform.arch() == Some(arch)),
                );
            }
        }

        let mut variant_map = serde_json::Map::new();
        for (key, value) in &self.variant {
            variant_map.insert(key.normalize(), value.to_json());
        }
        ctx.insert(
            "variant".to_string(),
            serde_json::Value::Object(variant_map),
        );

        serde_json::Value::Object(ctx)
    }
}
