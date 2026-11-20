//! Construct a [`PackageCache`] from the shared rattler configuration.

use std::path::PathBuf;

use rattler_config::config::{
    CommonConfig,
    package_cache::{PackageCacheLayerConfig, PackageCacheValidation},
};

use super::{PackageCache, PackageCacheLayer};
use crate::validation::ValidationMode;

impl From<PackageCacheValidation> for ValidationMode {
    fn from(validation: PackageCacheValidation) -> Self {
        match validation {
            PackageCacheValidation::Skip => ValidationMode::Skip,
            PackageCacheValidation::Fast => ValidationMode::Fast,
            PackageCacheValidation::Full => ValidationMode::Full,
        }
    }
}

impl PackageCacheLayer {
    fn from_config(layer: &PackageCacheLayerConfig, default_validation: ValidationMode) -> Self {
        let mut result = PackageCacheLayer::new(&layer.path)
            .with_read_only(layer.read_only)
            .with_validation_mode(layer.validation.map_or(default_validation, Into::into));
        result.filter.includes.clone_from(&layer.include);
        result.filter.excludes.clone_from(&layer.exclude);
        result
    }
}

impl PackageCache {
    /// Constructs a cache from the `package-cache` section of the shared
    /// rattler configuration (see [`rattler_config`]).
    ///
    /// The configured layers are queried in order, followed by
    /// `default_path`, the tool's own package cache. A package that is not
    /// found in any layer is written to the first layer that accepts it and is
    /// writable. Without configured layers this is [`PackageCache::new`] with
    /// the configured validation.
    ///
    /// Accepts a [`CommonConfig`]; a `&ConfigBase<T>` of any extension
    /// coerces into it.
    pub fn from_config(config: &CommonConfig, default_path: impl Into<PathBuf>) -> Self {
        let section = &config.package_cache;
        let validation = section
            .validation
            .map_or_else(ValidationMode::default, Into::into);
        let layers = section
            .layers
            .iter()
            .map(|layer| PackageCacheLayer::from_config(layer, validation))
            .chain(std::iter::once(
                PackageCacheLayer::new(default_path).with_validation_mode(validation),
            ));
        Self::from_layers(layers, false)
    }
}

#[cfg(test)]
mod tests {
    use rattler_config::{ConfigBase, NoExtension};

    use super::*;

    #[test]
    fn layers_follow_configuration_then_default() {
        let (config, unused) = ConfigBase::<NoExtension>::from_toml_str(
            r#"
[package-cache]
validation = "fast"

[[package-cache.layers]]
path = "/shared/pkgs"
read-only = true

[[package-cache.layers]]
path = "/team/pkgs"
include = ["pytorch"]
exclude = ["pytorch *cpu*"]
validation = "full"
"#,
        )
        .unwrap();
        assert!(unused.is_empty(), "{unused:?}");

        let cache = PackageCache::from_config(&config, "/home/me/pkgs");
        let [shared, team, default] = cache.layers() else {
            panic!("expected three layers");
        };

        assert_eq!(shared.path(), std::path::Path::new("/shared/pkgs"));
        // `/shared/pkgs` does not exist, so this can only come from the flag.
        assert!(shared.is_readonly());
        assert_eq!(shared.validation_mode(), ValidationMode::Fast);

        assert_eq!(team.validation_mode(), ValidationMode::Full);
        assert!(!team.is_readonly());
        assert_eq!(
            team.filter().includes(),
            ["pytorch".parse().unwrap()].as_slice()
        );
        assert_eq!(
            team.filter().excludes(),
            ["pytorch *cpu*".parse().unwrap()].as_slice()
        );

        assert_eq!(default.path(), std::path::Path::new("/home/me/pkgs"));
        assert_eq!(default.validation_mode(), ValidationMode::Fast);
        assert!(default.filter().includes().is_empty());
        assert!(!default.is_readonly());
    }
}
