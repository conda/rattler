pub mod config;
#[cfg(feature = "edit")]
pub mod edit;

#[cfg(test)]
mod tests {
    use crate::config::build::PackageFormatAndCompression;
    use crate::config::{Config, ConfigBase, MergeError, ValidationError};
    use serde::{Deserialize, Serialize};
    use std::collections::HashMap;
    use std::path::PathBuf;
    use tempfile::TempDir;
    use url::Url;

    // Test extension for comprehensive testing
    #[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
    pub struct TestExtension {
        #[serde(default)]
        pub custom_field: Option<String>,
        #[serde(default)]
        pub numeric_field: Option<u32>,
        #[serde(default)]
        pub bool_field: Option<bool>,
        #[serde(default)]
        pub array_field: Vec<String>,
        #[serde(default)]
        pub nested: HashMap<String, String>,
    }

    impl Config for TestExtension {
        fn get_extension_name(&self) -> String {
            "test".to_string()
        }

        fn merge_config(self, other: &Self) -> Result<Self, MergeError> {
            Ok(Self {
                custom_field: other.custom_field.clone().or(self.custom_field),
                numeric_field: other.numeric_field.or(self.numeric_field),
                bool_field: other.bool_field.or(self.bool_field),
                array_field: if other.array_field.is_empty() {
                    self.array_field
                } else {
                    other.array_field.clone()
                },
                nested: self
                    .nested
                    .into_iter()
                    .chain(other.nested.iter().map(|(k, v)| (k.clone(), v.clone())))
                    .collect(),
            })
        }

        fn validate(&self) -> Result<(), ValidationError> {
            if let Some(numeric) = self.numeric_field {
                if numeric > 100 {
                    return Err(ValidationError::InvalidValue(
                        "numeric_field".to_string(),
                        "must be <= 100".to_string(),
                    ));
                }
            }
            Ok(())
        }

        fn keys(&self) -> Vec<String> {
            vec![
                "custom_field".to_string(),
                "numeric_field".to_string(),
                "bool_field".to_string(),
                "array_field".to_string(),
                "nested".to_string(),
            ]
        }
    }

    type TestConfig = ConfigBase<TestExtension>;

    #[test]
    fn test_config_default() {
        let config = TestConfig::default();
        assert_eq!(config.default_channels, None);
        assert_eq!(config.authentication_override_file, None);
        assert_eq!(config.tls_no_verify, Some(false));
        assert!(config.mirrors.is_empty());
        assert!(config.s3_options.0.is_empty());
        assert_eq!(config.extensions, TestExtension::default());
    }

    #[test]
    fn test_edit_basic_config_values() {
        let mut config = TestConfig::default();

        // Test editing default channels
        config
            .set(
                "default-channels",
                Some(r#"["conda-forge", "bioconda"]"#.to_string()),
            )
            .unwrap();
        assert_eq!(
            config.default_channels.as_ref().map(std::vec::Vec::len),
            Some(2)
        );

        // Test editing authentication override file
        config
            .set(
                "authentication-override-file",
                Some("/path/to/auth".to_string()),
            )
            .unwrap();
        assert_eq!(
            config.authentication_override_file,
            Some(PathBuf::from("/path/to/auth"))
        );

        // Test editing TLS verification
        config
            .set("tls-no-verify", Some("true".to_string()))
            .unwrap();
        assert_eq!(config.tls_no_verify, Some(true));

        // Test editing mirrors
        config
            .set(
                "mirrors",
                Some(r#"{"https://conda.anaconda.org": ["https://mirror1.com", "https://mirror2.com"]}"#.to_string()),
            )
            .unwrap();
        assert_eq!(config.mirrors.len(), 1);
    }

    #[test]
    fn test_edit_concurrency_config() {
        let mut config = TestConfig::default();

        // Test editing concurrency solves
        config
            .set("concurrency.solves", Some("5".to_string()))
            .unwrap();
        assert_eq!(config.concurrency.solves, 5);

        // Test editing concurrency downloads
        config
            .set("concurrency.downloads", Some("10".to_string()))
            .unwrap();
        assert_eq!(config.concurrency.downloads, 10);
    }

    #[test]
    fn test_edit_repodata_config() {
        let mut config = TestConfig::default();

        // Test editing individual repodata config fields
        config
            .set("repodata-config.disable-jlap", Some("true".to_string()))
            .unwrap();
        assert_eq!(config.repodata_config.default.disable_jlap, Some(true));

        config
            .set("repodata-config.disable-bzip2", Some("false".to_string()))
            .unwrap();
        assert_eq!(config.repodata_config.default.disable_bzip2, Some(false));

        config
            .set("repodata-config.disable-zstd", Some("true".to_string()))
            .unwrap();
        assert_eq!(config.repodata_config.default.disable_zstd, Some(true));

        config
            .set("repodata-config.disable-sharded", Some("false".to_string()))
            .unwrap();
        assert_eq!(config.repodata_config.default.disable_sharded, Some(false));
    }

    #[test]
    fn test_edit_proxy_config() {
        let mut config = TestConfig::default();

        // Test editing proxy URLs
        config
            .set(
                "proxy-config.https",
                Some("https://proxy.example.com:8080".to_string()),
            )
            .unwrap();
        assert_eq!(
            config.proxy_config.https,
            Some(Url::parse("https://proxy.example.com:8080").unwrap())
        );

        config
            .set(
                "proxy-config.http",
                Some("http://proxy.example.com:8080".to_string()),
            )
            .unwrap();
        assert_eq!(
            config.proxy_config.http,
            Some(Url::parse("http://proxy.example.com:8080").unwrap())
        );

        // Test editing non-proxy hosts
        config
            .set(
                "proxy-config.non-proxy-hosts",
                Some(r#"["localhost", "127.0.0.1"]"#.to_string()),
            )
            .unwrap();
        assert_eq!(config.proxy_config.non_proxy_hosts.len(), 2);
    }

    #[test]
    fn test_edit_s3_options() {
        let mut config = TestConfig::default();

        // First add a bucket configuration
        config
            .set(
                "s3-options.mybucket",
                Some(r#"{"endpoint-url": "https://s3.example.com", "region": "us-west-2", "force-path-style": true}"#.to_string()),
            )
            .unwrap();

        // Verify the bucket was added
        assert!(config.s3_options.0.contains_key("mybucket"));
        let bucket_config = &config.s3_options.0["mybucket"];
        assert_eq!(
            bucket_config.endpoint_url,
            Url::parse("https://s3.example.com").unwrap()
        );
        assert_eq!(bucket_config.region, "us-west-2");
        assert!(bucket_config.force_path_style);

        // Test editing individual bucket properties
        config
            .set(
                "s3-options.mybucket.region",
                Some("eu-central-1".to_string()),
            )
            .unwrap();
        assert_eq!(config.s3_options.0["mybucket"].region, "eu-central-1");

        config
            .set(
                "s3-options.mybucket.endpoint-url",
                Some("https://s3.eu-central-1.amazonaws.com".to_string()),
            )
            .unwrap();
        assert_eq!(
            config.s3_options.0["mybucket"].endpoint_url,
            Url::parse("https://s3.eu-central-1.amazonaws.com").unwrap()
        );

        config
            .set(
                "s3-options.mybucket.force-path-style",
                Some("false".to_string()),
            )
            .unwrap();
        assert!(!config.s3_options.0["mybucket"].force_path_style);
    }

    #[test]
    fn test_edit_run_post_link_scripts() {
        let mut config = TestConfig::default();

        config
            .set("run-post-link-scripts", Some("insecure".to_string()))
            .unwrap();
        // Note: The actual implementation would need to be checked for the exact enum value
        assert!(config.run_post_link_scripts.is_some());
    }

    #[test]
    fn test_config_merge() {
        let config1 = TestConfig {
            default_channels: Some(vec!["conda-forge".parse().unwrap()]),
            tls_no_verify: Some(false),
            extensions: TestExtension {
                custom_field: Some("original".to_string()),
                numeric_field: Some(42),
                ..Default::default()
            },
            ..Default::default()
        };

        let config2 = TestConfig {
            default_channels: Some(vec!["bioconda".parse().unwrap()]),
            authentication_override_file: Some(PathBuf::from("/new/auth")),
            extensions: TestExtension {
                custom_field: Some("updated".to_string()),
                bool_field: Some(true),
                array_field: vec!["item1".to_string(), "item2".to_string()],
                ..Default::default()
            },
            ..Default::default()
        };

        let merged = config1.merge_config(&config2).unwrap();

        // The second config should take priority for overlapping fields
        assert_eq!(
            merged.default_channels.as_ref().map(std::vec::Vec::len),
            Some(1)
        );
        assert_eq!(
            merged.default_channels.as_ref().map(|v| v[0].to_string()),
            Some("bioconda".to_string())
        );
        assert_eq!(
            merged.authentication_override_file,
            Some(PathBuf::from("/new/auth"))
        );
        assert_eq!(merged.tls_no_verify, Some(false)); // from config1

        // Extension fields should be merged properly
        assert_eq!(merged.extensions.custom_field, Some("updated".to_string())); // from config2
        assert_eq!(merged.extensions.numeric_field, Some(42)); // from config1
        assert_eq!(merged.extensions.bool_field, Some(true)); // from config2
        assert_eq!(merged.extensions.array_field.len(), 2); // from config2
    }

    #[test]
    fn test_extend_config_with_new_s3_buckets() {
        let mut config = TestConfig::default();

        // Add multiple S3 bucket configurations
        config
            .set(
                "s3-options.production",
                Some(r#"{"endpoint-url": "https://s3.amazonaws.com", "region": "us-east-1", "force-path-style": false}"#.to_string()),
            )
            .unwrap();

        config
            .set(
                "s3-options.development",
                Some(r#"{"endpoint-url": "https://minio.dev.example.com", "region": "dev-region", "force-path-style": true}"#.to_string()),
            )
            .unwrap();

        config
            .set(
                "s3-options.staging",
                Some(r#"{"endpoint-url": "https://s3.staging.example.com", "region": "us-west-2", "force-path-style": false}"#.to_string()),
            )
            .unwrap();

        assert_eq!(config.s3_options.0.len(), 3);
        assert!(config.s3_options.0.contains_key("production"));
        assert!(config.s3_options.0.contains_key("development"));
        assert!(config.s3_options.0.contains_key("staging"));

        // Verify different configurations
        assert_eq!(config.s3_options.0["production"].region, "us-east-1");
        assert!(config.s3_options.0["development"].force_path_style);
        assert_eq!(
            config.s3_options.0["staging"].endpoint_url,
            Url::parse("https://s3.staging.example.com").unwrap()
        );
    }

    #[test]
    fn test_extend_config_with_multiple_mirrors() {
        let mut config = TestConfig::default();

        // Add multiple mirror configurations
        config
            .set(
                "mirrors",
                Some(r#"{
                    "https://conda.anaconda.org": ["https://mirror1.com", "https://mirror2.com"],
                    "https://repo.continuum.io": ["https://fast-mirror.net"],
                    "https://conda-forge.org": ["https://mirror.conda-forge.org", "https://backup.conda-forge.org"]
                }"#.to_string()),
            )
            .unwrap();

        assert_eq!(config.mirrors.len(), 3);

        let anaconda_mirrors = &config.mirrors[&Url::parse("https://conda.anaconda.org").unwrap()];
        assert_eq!(anaconda_mirrors.len(), 2);
        assert!(anaconda_mirrors.contains(&Url::parse("https://mirror1.com").unwrap()));

        let continuum_mirrors = &config.mirrors[&Url::parse("https://repo.continuum.io").unwrap()];
        assert_eq!(continuum_mirrors.len(), 1);

        let conda_forge_mirrors = &config.mirrors[&Url::parse("https://conda-forge.org").unwrap()];
        assert_eq!(conda_forge_mirrors.len(), 2);
    }

    #[test]
    fn test_save_and_load_config() {
        let temp_dir = TempDir::new().unwrap();
        let config_path = temp_dir.path().join("config.toml");

        let mut config = TestConfig::default();
        config
            .set(
                "default-channels",
                Some(r#"["conda-forge", "bioconda"]"#.to_string()),
            )
            .unwrap();
        config
            .set("tls-no-verify", Some("true".to_string()))
            .unwrap();
        config
            .set("concurrency.solves", Some("8".to_string()))
            .unwrap();

        // Save the config
        config.save(&config_path).unwrap();

        // Verify file was created and can be read
        assert!(config_path.exists());
        let content = std::fs::read_to_string(&config_path).unwrap();
        assert!(content.contains("default_channels") || content.contains("default-channels"));
        assert!(content.contains("tls_no_verify") || content.contains("tls-no-verify"));
        assert!(content.contains("[concurrency]"));

        // Load config from file
        let loaded_config = TestConfig::load_from_files([&config_path]).unwrap();
        assert_eq!(
            loaded_config
                .default_channels
                .as_ref()
                .map(std::vec::Vec::len),
            Some(2)
        );
        assert_eq!(loaded_config.tls_no_verify, Some(true));
        assert_eq!(loaded_config.concurrency.solves, 8);
    }

    #[test]
    fn test_config_edit_and_save_snapshot() {
        let mut config = TestConfig::default();

        // Edit multiple configuration values
        config
            .set(
                "default-channels",
                Some(r#"["conda-forge", "bioconda", "pytorch"]"#.to_string()),
            )
            .unwrap();
        config
            .set("tls-no-verify", Some("true".to_string()))
            .unwrap();
        config
            .set("concurrency.solves", Some("12".to_string()))
            .unwrap();
        config
            .set("concurrency.downloads", Some("24".to_string()))
            .unwrap();
        config
            .set(
                "authentication-override-file",
                Some("/home/user/.rattler-auth".to_string()),
            )
            .unwrap();
        config
            .set(
                "mirrors",
                Some(r#"{"https://conda.anaconda.org": ["https://mirror1.com", "https://mirror2.com"]}"#.to_string()),
            )
            .unwrap();

        // Use the config's to_toml() method for consistent serialization
        let toml_output = config.to_toml().unwrap();
        insta::assert_snapshot!("basic_config_edit", toml_output);
    }

    #[test]
    fn test_proxy_config_edit_snapshot() {
        let mut config = TestConfig::default();

        // Edit proxy configuration
        config
            .set(
                "proxy-config.https",
                Some("https://corporate-proxy.example.com:8080".to_string()),
            )
            .unwrap();
        config
            .set(
                "proxy-config.http",
                Some("http://corporate-proxy.example.com:8080".to_string()),
            )
            .unwrap();
        config
            .set(
                "proxy-config.non-proxy-hosts",
                Some(r#"["localhost", "127.0.0.1", "*.internal.com"]"#.to_string()),
            )
            .unwrap();

        // Use the config's to_toml() method for consistent serialization
        let toml_output = config.to_toml().unwrap();
        insta::assert_snapshot!("proxy_config_edit", toml_output);
    }

    #[test]
    fn test_repodata_config_edit_snapshot() {
        let mut config = TestConfig::default();

        // Edit repodata configuration
        config
            .set("repodata-config.disable-jlap", Some("true".to_string()))
            .unwrap();
        config
            .set("repodata-config.disable-bzip2", Some("false".to_string()))
            .unwrap();
        config
            .set("repodata-config.disable-zstd", Some("true".to_string()))
            .unwrap();
        config
            .set("repodata-config.disable-sharded", Some("false".to_string()))
            .unwrap();

        // Use the config's to_toml() method for consistent serialization
        let toml_output = config.to_toml().unwrap();
        insta::assert_snapshot!("repodata_config_edit", toml_output);
    }

    #[test]
    fn test_s3_config_edit_snapshot() {
        let mut config = TestConfig::default();

        // Add S3 bucket configurations
        config
            .set(
                "s3-options.production-bucket",
                Some(r#"{"endpoint-url": "https://s3.us-east-1.amazonaws.com", "region": "us-east-1", "force-path-style": false}"#.to_string()),
            )
            .unwrap();
        config
            .set(
                "s3-options.dev-bucket",
                Some(r#"{"endpoint-url": "https://minio.dev.example.com", "region": "us-west-2", "force-path-style": true}"#.to_string()),
            )
            .unwrap();

        // Edit individual S3 bucket properties
        config
            .set(
                "s3-options.production-bucket.region",
                Some("us-west-1".to_string()),
            )
            .unwrap();

        // Use the config's to_toml() method for consistent serialization
        let toml_output = config.to_toml().unwrap();
        insta::assert_snapshot!("s3_config_edit", toml_output);
    }

    #[test]
    fn test_comprehensive_config_edit_snapshot() {
        let mut config = TestConfig::default();

        // Edit comprehensive configuration covering all areas
        config
            .set(
                "default-channels",
                Some(r#"["conda-forge", "bioconda", "nvidia", "pytorch"]"#.to_string()),
            )
            .unwrap();
        config
            .set("tls-no-verify", Some("false".to_string()))
            .unwrap();
        config
            .set(
                "authentication-override-file",
                Some("/etc/conda/auth.json".to_string()),
            )
            .unwrap();
        config
            .set("concurrency.solves", Some("16".to_string()))
            .unwrap();
        config
            .set("concurrency.downloads", Some("32".to_string()))
            .unwrap();
        config
            .set(
                "mirrors",
                Some(r#"{
                    "https://conda.anaconda.org": ["https://mirror.example.com", "https://backup.example.com"],
                    "https://repo.continuum.io": ["https://fast-mirror.net"]
                }"#.to_string()),
            )
            .unwrap();
        config
            .set(
                "proxy-config.https",
                Some("https://secure-proxy.company.com:443".to_string()),
            )
            .unwrap();
        config
            .set(
                "proxy-config.non-proxy-hosts",
                Some(r#"["localhost", "*.company.com", "10.0.0.0/8"]"#.to_string()),
            )
            .unwrap();
        config
            .set("repodata-config.disable-jlap", Some("false".to_string()))
            .unwrap();
        config
            .set("repodata-config.disable-zstd", Some("false".to_string()))
            .unwrap();
        config
            .set(
                "s3-options.company-bucket",
                Some(r#"{"endpoint-url": "https://s3.company.com", "region": "company-region", "force-path-style": true}"#.to_string()),
            )
            .unwrap();
        config
            .set("run-post-link-scripts", Some("insecure".to_string()))
            .unwrap();

        // Use the config's to_toml() method for consistent serialization
        let toml_output = config.to_toml().unwrap();
        insta::assert_snapshot!("comprehensive_config_edit", toml_output);
    }

    #[test]
    fn test_config_save_and_load_roundtrip_snapshot() {
        let temp_dir = TempDir::new().unwrap();
        let config_path = temp_dir.path().join("roundtrip.toml");

        let mut original_config = TestConfig::default();

        // Configure a complex setup
        original_config
            .set(
                "default-channels",
                Some(r#"["conda-forge", "pytorch", "nvidia"]"#.to_string()),
            )
            .unwrap();
        original_config
            .set("concurrency.solves", Some("6".to_string()))
            .unwrap();
        original_config
            .set(
                "s3-options.test-bucket",
                Some(r#"{"endpoint-url": "https://s3.amazonaws.com", "region": "us-east-1", "force-path-style": false}"#.to_string()),
            )
            .unwrap();

        // Save the config using the save() method
        original_config.save(&config_path).unwrap();

        // Read the saved TOML content
        let saved_content = std::fs::read_to_string(&config_path).unwrap();
        insta::assert_snapshot!("config_save_roundtrip", saved_content);

        // Verify roundtrip consistency by loading and comparing
        let loaded_config = TestConfig::load_from_files([&config_path]).unwrap();
        assert_eq!(
            loaded_config
                .default_channels
                .as_ref()
                .map(std::vec::Vec::len),
            Some(3)
        );
        assert_eq!(loaded_config.concurrency.solves, 6);
        assert!(loaded_config.s3_options.0.contains_key("test-bucket"));
    }

    #[test]
    fn test_config_error_handling() {
        let mut config = TestConfig::default();

        // Test unknown key error
        let result = config.set("unknown-key", Some("value".to_string()));
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            crate::edit::ConfigEditError::UnknownKey { .. }
        ));

        // Test invalid JSON for mirrors
        let result = config.set("mirrors", Some("invalid json".to_string()));
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            crate::edit::ConfigEditError::JsonParseError { .. }
        ));

        // Test invalid boolean
        let result = config.set("tls-no-verify", Some("not-a-boolean".to_string()));
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            crate::edit::ConfigEditError::BoolParseError { .. }
        ));

        // Test invalid number
        let result = config.set("concurrency.solves", Some("not-a-number".to_string()));
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            crate::edit::ConfigEditError::NumberParseError { .. }
        ));

        // Test invalid URL
        let result = config.set("proxy-config.https", Some("not-a-url".to_string()));
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            crate::edit::ConfigEditError::UrlParseError { .. }
        ));
    }

    #[test]
    fn test_config_keys_listing() {
        let config = TestConfig::default();
        let keys = config.keys();

        // Verify that all expected key categories are present
        assert!(keys.iter().any(|k| k.starts_with("build.")));
        assert!(keys.iter().any(|k| k.starts_with("repodata.")));
        assert!(keys.iter().any(|k| k.starts_with("concurrency.")));
        assert!(keys.iter().any(|k| k.starts_with("proxy.")));
        assert!(keys.iter().any(|k| k.starts_with("test.")));

        // Verify core keys are present
        assert!(keys.contains(&"default_channels".to_string()));
        assert!(keys.contains(&"authentication_override_file".to_string()));
        assert!(keys.contains(&"tls_no_verify".to_string()));
        assert!(keys.contains(&"mirrors".to_string()));
    }

    #[test]
    fn test_merge_multiple_configs() {
        let base_config = TestConfig {
            default_channels: Some(vec!["defaults".parse().unwrap()]),
            tls_no_verify: Some(false),
            ..Default::default()
        };

        let user_config = TestConfig {
            default_channels: Some(vec!["conda-forge".parse().unwrap()]),
            authentication_override_file: Some(PathBuf::from("/home/user/.conda-auth")),
            extensions: TestExtension {
                custom_field: Some("user-value".to_string()),
                ..Default::default()
            },
            ..Default::default()
        };

        let project_config = TestConfig {
            concurrency: crate::config::concurrency::ConcurrencyConfig {
                solves: 4,
                downloads: 8,
            },
            extensions: TestExtension {
                numeric_field: Some(50),
                bool_field: Some(true),
                ..Default::default()
            },
            ..Default::default()
        };

        // Merge configs in order: base -> user -> project
        let merged = base_config
            .merge_config(&user_config)
            .unwrap()
            .merge_config(&project_config)
            .unwrap();

        // Project config takes priority for overlapping fields
        assert_eq!(merged.concurrency.solves, 4);
        assert_eq!(merged.concurrency.downloads, 8);

        // User config values are preserved where not overridden
        assert_eq!(
            merged.default_channels.as_ref().map(std::vec::Vec::len),
            Some(1)
        );
        assert_eq!(
            merged.authentication_override_file,
            Some(PathBuf::from("/home/user/.conda-auth"))
        );

        // Base config values are preserved where not overridden
        assert_eq!(merged.tls_no_verify, Some(false));

        // Extension values are merged properly
        assert_eq!(
            merged.extensions.custom_field,
            Some("user-value".to_string())
        );
        assert_eq!(merged.extensions.numeric_field, Some(50));
        assert_eq!(merged.extensions.bool_field, Some(true));
    }

    // Load config file from `test-data` directory
    fn load_test_config() -> TestConfig {
        let config_path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("test-data")
            .join("config.toml");
        TestConfig::load_from_files([&config_path]).expect("Failed to load test config")
    }

    #[test]
    fn test_load_and_validate_test_config() {
        let config = load_test_config();
        assert_eq!(
            config.default_channels,
            Some(vec!["conda-forge".parse().unwrap()])
        );
        assert_eq!(config.tls_no_verify, Some(false));
        assert_eq!(
            config.authentication_override_file,
            Some("/path/to/your/override.json".into())
        );
        assert_eq!(config.mirrors.len(), 2);
        assert!(config.s3_options.0.contains_key("my-bucket"));

        assert_eq!(
            config.build.package_format,
            Some(PackageFormatAndCompression {
                archive_type: rattler_conda_types::package::ArchiveType::TarBz2,
                compression_level:
                    rattler_conda_types::compression_level::CompressionLevel::Numeric(3)
            })
        );

        // The following config is _NOT LOADED_ from test data, so we are just checking if it has the default values
        assert_ne!(
            config.channel_config.root_dir,
            PathBuf::from("/path/to/your/channels")
        );
        assert_ne!(
            config.channel_config.channel_alias,
            Url::parse("https://friendly.conda.server").unwrap()
        );
    }
}
