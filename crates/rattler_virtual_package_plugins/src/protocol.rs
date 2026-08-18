//! The report a detection plugin writes to stdout.
//!
//! One JSON object, keyed by virtual package name:
//!
//! ```json
//! {
//!   "version": 1,
//!   "virtual_packages": {
//!     "__cuda": { "version": "12.4" },
//!     "__cuda_arch": { "version": "0", "build_string": "sm_89" },
//!     "__rocm": null
//!   },
//!   "cache": {
//!     "ttl_seconds": 86400,
//!     "watch_paths": ["/sys/module/amdgpu/version"],
//!     "watch_env": ["CUDA_VISIBLE_DEVICES"]
//!   }
//! }
//! ```
//!
//! `null` is how a plugin says "not on this system". A plugin has to give a
//! verdict on every virtual package its channel registered it for, so absence
//! must be something it can state; silence is a contract violation instead. That
//! distinction survives here because the contract checks the *keys*: a missing
//! key is silence, an explicit `null` is a verdict, and no deserializer subtlety
//! stands between the two.
//!
//! Keying by name also makes a duplicate verdict impossible to write down, which
//! is why nothing below has to detect one.
//!
//! Unknown keys are ignored rather than rejected. A plugin written against a
//! newer protocol than the client understands should still be usable for the
//! part they agree on, and rejecting it would buy no safety: the plugin is
//! arbitrary code the client just ran.
//!
//! The `version` of the report itself is the exception, and is why it exists:
//! an added key can be ignored, while a key whose *meaning* changed cannot be
//! told from the one this client knows. So a report is read when it says it
//! speaks [`PROTOCOL_VERSION`] and refused otherwise, which is a plugin that
//! needs a newer client rather than a plugin that is broken.

use std::{collections::BTreeMap, time::Duration};

use rattler_conda_types::{GenericVirtualPackage, PackageName, Version};
use serde::{Deserialize, Serialize};

/// The revision of this protocol a report is read as.
pub const PROTOCOL_VERSION: u64 = 1;

/// Everything one plugin run reported.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct PluginReport {
    /// Which revision of this protocol the plugin wrote. Always
    /// [`PROTOCOL_VERSION`]: a report saying anything else is refused rather
    /// than guessed at.
    pub version: u64,

    /// One entry per virtual package the plugin gave a verdict about, `None`
    /// where it reported the virtual package absent.
    pub virtual_packages: BTreeMap<PackageName, Option<Detected>>,

    /// How long these verdicts may be reused, if the plugin said.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache: Option<CachePolicy>,

    /// Keys this client does not know, kept only to report them.
    #[serde(flatten)]
    unknown: BTreeMap<String, serde_json::Value>,
}

impl PluginReport {
    /// The solver-facing form of every virtual package reported present.
    pub fn present(&self) -> impl Iterator<Item = GenericVirtualPackage> + '_ {
        self.virtual_packages.iter().filter_map(|(name, detected)| {
            let detected = detected.as_ref()?;
            Some(GenericVirtualPackage {
                name: name.clone(),
                version: detected.version.clone(),
                build_string: detected.build_string.clone().unwrap_or_default(),
            })
        })
    }

    /// Every key of the report this client does not understand, including the
    /// ones inside the cache policy.
    ///
    /// A misspelled key is otherwise invisible: `ttl_secnods` would silently
    /// mean "no expiry" rather than the day the plugin author meant.
    fn unknown_keys(&self) -> impl Iterator<Item = &str> {
        let cache = self.cache.iter().flat_map(|cache| cache.unknown.keys());
        self.unknown.keys().chain(cache).map(String::as_str)
    }
}

/// What a plugin found for a virtual package that is present.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct Detected {
    /// The detected version.
    pub version: Version,

    /// The build string, for virtual packages that carry their information
    /// there rather than in the version. `__archspec` is the case CEP 30
    /// requires it for; `__cuda_arch` is *not* one, despite the name -- CEP 46
    /// puts its compute capability in the version and fixes its build string at
    /// `0`, having rejected using the build string for device identity so that
    /// nobody writes a constraint against it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub build_string: Option<String>,
}

/// How long a set of verdicts may be reused before the plugin must run again.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq, Default)]
pub struct CachePolicy {
    /// Seconds the verdicts stay valid, as the plugin asked. What is actually
    /// applied is [`CachePolicy::effective_ttl_seconds`]: a plugin that says
    /// nothing still gets an expiry, and one that asks for a year does not.
    #[serde(default)]
    pub ttl_seconds: Option<u64>,

    /// Paths whose existence or modification time invalidates the verdicts.
    /// This is what catches a driver upgrade between two solves.
    #[serde(default)]
    pub watch_paths: Vec<String>,

    /// Environment variables whose value or absence invalidates the verdicts.
    /// This is what catches the driver still being there but the user having
    /// hidden it, which no path can show.
    #[serde(default)]
    pub watch_env: Vec<String>,

    /// Keys this client does not know, kept only to report them.
    #[serde(flatten)]
    unknown: BTreeMap<String, serde_json::Value>,
}

impl CachePolicy {
    /// How long verdicts last when the plugin does not say.
    ///
    /// A plugin that declares no policy is the one that thought least about
    /// caching, so it must not be the one whose answers are kept longest. An
    /// hour bounds how stale a verdict can be while costing at most one plugin
    /// run per hour -- against a prefix that already exists, that is
    /// milliseconds.
    pub const DEFAULT_TTL: Duration = Duration::from_secs(60 * 60);

    /// The longest a plugin may ask for.
    ///
    /// Without a ceiling a channel could pin a verdict on a machine for as long
    /// as it liked, and a driver upgrade would go unnoticed until someone
    /// cleared the cache by hand. Thirty days is long enough for something that
    /// genuinely never changes and short enough to self-heal.
    pub const MAX_TTL: Duration = Duration::from_secs(30 * 24 * 60 * 60);

    /// The expiry to actually record, in seconds.
    ///
    /// Every entry gets one. "Cache these forever" is not something a plugin can
    /// ask for: `watch_paths` and `watch_env` make an entry expire *sooner* than
    /// its TTL, never later.
    pub fn effective_ttl_seconds(&self) -> u64 {
        let asked = self
            .ttl_seconds
            .unwrap_or_else(|| Self::DEFAULT_TTL.as_secs());
        if asked > Self::MAX_TTL.as_secs() {
            tracing::debug!(
                "a plugin asked for a cache lifetime of {asked}s; using the maximum of {}s",
                Self::MAX_TTL.as_secs()
            );
        }
        asked.min(Self::MAX_TTL.as_secs())
    }
}

/// A plugin wrote something that is not a report.
#[derive(Debug, thiserror::Error)]
pub enum ProtocolError {
    /// The plugin exited successfully without writing anything.
    #[error("the plugin wrote no report to stdout")]
    Empty,

    /// What the plugin wrote could not be read as a report.
    #[error("the plugin's report could not be read")]
    Malformed(#[from] serde_json::Error),

    /// The plugin speaks a revision of this protocol that this client does not.
    #[error(
        "the plugin wrote a version {version} report, and this client reads version \
         {PROTOCOL_VERSION}"
    )]
    UnsupportedVersion {
        /// The revision the plugin said it wrote.
        version: u64,
    },
}

/// Parse what a plugin wrote to stdout.
///
/// Surrounding whitespace does not matter, so a trailing newline or a shell
/// script's padding is fine. Keys this client does not understand are logged and
/// ignored.
pub fn parse_report(stdout: &str) -> Result<PluginReport, ProtocolError> {
    if stdout.trim().is_empty() {
        return Err(ProtocolError::Empty);
    }

    let report: PluginReport = serde_json::from_str(stdout)?;
    if report.version != PROTOCOL_VERSION {
        return Err(ProtocolError::UnsupportedVersion {
            version: report.version,
        });
    }
    for key in report.unknown_keys() {
        tracing::debug!("ignoring '{key}', which a plugin report is not expected to carry");
    }

    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn detected(report: &PluginReport, name: &str) -> Option<String> {
        report
            .present()
            .find(|package| package.name.as_source() == name)
            .map(|package| package.to_string())
    }

    #[test]
    fn parses_verdicts_and_a_cache_policy() {
        let stdout = r#"
        {
          "version": 1,
          "virtual_packages": {
            "__cuda": { "version": "12.4" },
            "__cuda_arch": { "version": "0", "build_string": "sm_89" },
            "__rocm": null
          },
          "cache": {
            "ttl_seconds": 86400,
            "watch_paths": ["/sys/module/amdgpu/version"],
            "watch_env": ["CUDA_VISIBLE_DEVICES"]
          }
        }
        "#;
        let report = parse_report(stdout).unwrap();

        assert_eq!(report.virtual_packages.len(), 3);
        assert_eq!(detected(&report, "__cuda").as_deref(), Some("__cuda=12.4"));
        assert_eq!(
            detected(&report, "__cuda_arch").as_deref(),
            Some("__cuda_arch=0=sm_89")
        );
        assert_eq!(
            detected(&report, "__rocm"),
            None,
            "a null verdict yields no virtual package"
        );

        let policy = report.cache.unwrap();
        assert_eq!(policy.ttl_seconds, Some(86400));
        assert_eq!(policy.watch_paths, ["/sys/module/amdgpu/version"]);
        assert_eq!(policy.watch_env, ["CUDA_VISIBLE_DEVICES"]);
    }

    #[test]
    fn a_report_without_virtual_packages_is_rejected() {
        assert!(matches!(
            parse_report(r#"{"version": 1}"#),
            Err(ProtocolError::Malformed(_))
        ));
    }

    #[test]
    fn a_verdict_without_a_version_is_rejected() {
        for stdout in [
            r#"{"version": 1, "virtual_packages": {"__cuda": {}}}"#,
            r#"{"version": 1, "virtual_packages": {"__cuda": {"build_string": "sm_89"}}}"#,
            r#"{"version": 1, "virtual_packages": {"__cuda": {"version": null}}}"#,
        ] {
            assert!(parse_report(stdout).is_err(), "should reject: {stdout}");
        }
    }

    #[test]
    fn unknown_keys_are_kept_out_of_the_way_rather_than_rejected() {
        let stdout = r#"
        {
          "version": 1,
          "virtual_packages": { "__cuda": { "version": "12.4", "vendor": "nvidia" } },
          "cache": { "ttl_seconds": 60, "watch_registry_keys": ["HKLM/nvidia"] },
          "reported_by": "nvidia-smi"
        }
        "#;
        let report = parse_report(stdout).unwrap();

        assert_eq!(detected(&report, "__cuda").as_deref(), Some("__cuda=12.4"));
        assert_eq!(report.cache.as_ref().unwrap().ttl_seconds, Some(60));
        assert_eq!(
            report.unknown_keys().collect::<Vec<_>>(),
            ["reported_by", "watch_registry_keys"],
            "unknown keys are collected so they can be reported"
        );
    }

    #[test]
    fn nothing_and_garbage_are_distinct_failures() {
        assert!(matches!(parse_report("  \n "), Err(ProtocolError::Empty)));
        assert!(matches!(
            parse_report("not json"),
            Err(ProtocolError::Malformed(_))
        ));
        assert!(
            matches!(
                parse_report(r#"{"version": 1, "virtual_packages": ["__cuda"]}"#),
                Err(ProtocolError::Malformed(_))
            ),
            "a list of names says nothing about presence"
        );
    }

    #[test]
    fn a_report_without_a_version_is_rejected() {
        assert!(matches!(
            parse_report(r#"{"virtual_packages": {}}"#),
            Err(ProtocolError::Malformed(_))
        ));
    }

    #[test]
    fn a_report_from_a_later_revision_is_refused_by_name() {
        let error = parse_report(r#"{"version": 2, "virtual_packages": {}}"#)
            .expect_err("this client reads version 1");
        assert!(
            matches!(error, ProtocolError::UnsupportedVersion { version: 2 }),
            "got: {error}"
        );
    }

    #[test]
    fn a_plugin_that_declares_no_policy_still_gets_an_expiry() {
        let no_cache_key = parse_report(r#"{"version": 1, "virtual_packages": {}}"#).unwrap();
        assert!(no_cache_key.cache.is_none());
        assert_eq!(
            CachePolicy::default().effective_ttl_seconds(),
            CachePolicy::DEFAULT_TTL.as_secs()
        );

        // A cache policy that only watches paths says nothing about time, and
        // gets the same treatment.
        let watching = parse_report(
            r#"{"version": 1, "virtual_packages": {}, "cache": {"watch_paths": ["/dev/null"]}}"#,
        )
        .unwrap();
        assert_eq!(
            watching.cache.unwrap().effective_ttl_seconds(),
            CachePolicy::DEFAULT_TTL.as_secs()
        );
    }

    #[test]
    fn a_plugin_cannot_ask_to_be_cached_forever() {
        let forever = parse_report(
            r#"{"version": 1, "virtual_packages": {}, "cache": {"ttl_seconds": 999999999}}"#,
        )
        .unwrap();
        assert_eq!(
            forever.cache.unwrap().effective_ttl_seconds(),
            CachePolicy::MAX_TTL.as_secs()
        );
    }

    #[test]
    fn a_ttl_within_the_maximum_is_honoured() {
        let hour = parse_report(
            r#"{"version": 1, "virtual_packages": {}, "cache": {"ttl_seconds": 3600}}"#,
        )
        .unwrap();
        assert_eq!(hour.cache.unwrap().effective_ttl_seconds(), 3600);

        // Zero is a plugin saying "do not reuse this", and is not overridden
        // into the default.
        let never =
            parse_report(r#"{"version": 1, "virtual_packages": {}, "cache": {"ttl_seconds": 0}}"#)
                .unwrap();
        assert_eq!(never.cache.unwrap().effective_ttl_seconds(), 0);
    }

    #[test]
    fn a_parse_failure_says_where() {
        let error =
            parse_report("{\n  \"version\": 1,\n  \"virtual_packages\": {\n    oops\n  }\n}")
                .unwrap_err();
        let ProtocolError::Malformed(source) = error else {
            panic!("expected a malformed report");
        };
        assert_eq!(source.line(), 4, "{source}");
    }
}
