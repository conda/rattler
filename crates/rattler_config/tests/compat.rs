//! Compatibility test catalog for the shared configuration.
//!
//! These tests pin down the *contract* of `rattler_config` so that consumers
//! (pixi, rattler-build, rattler-index) can upgrade without breaking their
//! users' existing `config.toml` files:
//!
//! 1. **Parsing permutations** — canonical kebab-case, legacy `snake_case`
//!    aliases, deprecated keys and typos (fixtures in `test-data/compat/`).
//!    Deprecated/unknown keys must parse with warnings, never hard errors.
//! 2. **Round-trip stability** — load → serialize → load is lossless and
//!    serialization is idempotent, so saving a config never corrupts it.
//! 3. **Editing matrix** — every known key can be `set` and `unset`; a
//!    set+unset cycle on a pristine config restores the default, proving
//!    edits have no collateral effect on unrelated keys.
//! 4. **Merge semantics** — how two layered files combine (replace vs
//!    extend vs recursive merge) is snapshotted per key family.

use std::collections::BTreeSet;
use std::path::PathBuf;

use rattler_config::{ConfigBase, NoExtension};

type Config = ConfigBase<NoExtension>;

fn fixture_path(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("test-data/compat")
        .join(name)
}

fn fixture(name: &str) -> String {
    fs_err::read_to_string(fixture_path(name)).unwrap()
}

fn parse(name: &str) -> (Config, BTreeSet<String>) {
    Config::from_toml_str(&fixture(name)).unwrap_or_else(|e| panic!("{name} must parse: {e}"))
}

/// Strip machine-dependent values before snapshotting: `concurrency.solves`
/// defaults to the local CPU count and `channel_config.root_dir` to the
/// working directory, so a raw snapshot would only pass on the machine that
/// generated it. `solves` is normalized *unconditionally* (a conditional
/// "only when it equals the default" would flip whenever an explicitly
/// configured value happens to match the CPU count); explicitly configured
/// values are asserted in `explicit_concurrency_values_parse` instead.
fn normalized(mut config: Config) -> Config {
    config.concurrency.solves = 0;
    config.channel_config =
        rattler_conda_types::ChannelConfig::default_with_root_dir(PathBuf::from("<normalized>"));
    config
}

/// Every fixture must parse; snapshot the parsed result and the reported
/// unused keys so schema changes are reviewed consciously.
#[test]
fn parsing_permutations() {
    for name in [
        "kitchen-sink.toml",
        "snake-case-aliases.toml",
        "deprecated-and-unknown.toml",
        "override-layer.toml",
    ] {
        let (config, unused) = parse(name);
        insta::assert_debug_snapshot!(format!("parse__{name}"), (unused, normalized(config)));
    }
}

/// Explicitly configured concurrency values parse correctly. Kept out of
/// the snapshots because `normalized` blanks `solves` (see there).
#[test]
fn explicit_concurrency_values_parse() {
    let (config, _) = parse("kitchen-sink.toml");
    assert_eq!(config.concurrency.solves, 4);
    assert_eq!(config.concurrency.downloads, 12);

    let (config, _) = parse("override-layer.toml");
    assert_eq!(config.concurrency.solves, 9);
}

/// Snake-case spellings must parse to the same configuration as their
/// kebab-case equivalents, without any unused-key warnings.
#[test]
fn snake_case_aliases_are_equivalent() {
    let (config, unused) = parse("snake-case-aliases.toml");
    assert!(unused.is_empty(), "aliases must not warn: {unused:?}");

    let canonical = r#"
        default-channels = ["conda-forge"]
        authentication-override-file = "/path/to/auth.json"
        tls-no-verify = true
        tls-root-certs = "system"
        allow-symbolic-links = false
        allow-hard-links = true
        allow-ref-links = false

        [repodata-config]
        disable-bzip2 = true
        disable-zstd = true
    "#;
    let (canonical, _) = Config::from_toml_str(canonical).unwrap();
    assert_eq!(config, canonical);
}

/// Deprecated keys and typos must never fail the load; they surface in the
/// unused-keys set (which consumers turn into warnings).
#[test]
fn deprecated_and_unknown_keys_warn_but_parse() {
    let (config, unused) = parse("deprecated-and-unknown.toml");

    // Values around the deprecated keys still load.
    assert_eq!(
        config.default_channels.as_ref().map(Vec::len),
        Some(1),
        "values next to unknown keys must survive"
    );
    assert_eq!(config.repodata_config.default.disable_bzip2, Some(true));
    // Legacy `"native"` resolves to the system store.
    assert_eq!(
        config.tls_root_certs,
        Some(rattler_config::config::tls::TlsRootCerts::System)
    );

    // Everything unknown is reported, at full depth.
    for key in [
        "definitely-a-typo",
        "pinning-strategy",
        "change-ps1",
        "repodata-config.disable-jlap",
    ] {
        assert!(
            unused.contains(key),
            "{key} must be reported, got {unused:?}"
        );
    }
}

/// load → serialize → load must be lossless, and serialization idempotent.
/// This is what guarantees that `config set` + `save` never corrupts the
/// rest of a user's configuration.
#[test]
fn round_trip_is_lossless_and_idempotent() {
    for name in [
        "kitchen-sink.toml",
        "snake-case-aliases.toml",
        "override-layer.toml",
    ] {
        let (config, _) = parse(name);
        let first = config.to_toml().unwrap();
        let (reloaded, unused) = Config::from_toml_str(&first).unwrap();
        assert!(
            unused.is_empty(),
            "{name}: serialization must not invent unknown keys: {unused:?}"
        );
        assert_eq!(config, reloaded, "{name}: round trip must be lossless");
        let second = reloaded.to_toml().unwrap();
        assert_eq!(first, second, "{name}: serialization must be idempotent");
    }
}

/// The editing matrix: one entry per known key family with a representative
/// value. Extend this list whenever a key is added to `CommonConfig`.
const EDIT_MATRIX: &[(&str, &str)] = &[
    ("default-channels", r#"["conda-forge"]"#),
    ("authentication-override-file", "/tmp/auth.json"),
    ("tls-no-verify", "true"),
    ("tls-root-certs", "webpki"),
    (
        "mirrors",
        r#"{"https://conda.anaconda.org/conda-forge": ["https://mirror.example.com/conda-forge"]}"#,
    ),
    ("run-post-link-scripts", "insecure"),
    ("allow-symbolic-links", "false"),
    ("allow-hard-links", "true"),
    ("allow-ref-links", "false"),
    ("build.package-format", "conda:max"),
    ("repodata-config.disable-bzip2", "true"),
    ("repodata-config.disable-zstd", "false"),
    ("repodata-config.disable-sharded", "true"),
    ("concurrency.solves", "3"),
    ("concurrency.downloads", "21"),
    ("proxy-config.https", "https://proxy.example.com:8080"),
    ("proxy-config.http", "http://proxy.example.com:8080"),
    ("proxy-config.non-proxy-hosts", r#"["localhost"]"#),
    (
        "s3-options.some-bucket",
        r#"{"endpoint-url": "https://s3.example.com", "region": "auto", "force-path-style": true}"#,
    ),
];

/// Every key in the matrix can be set on a fully populated config, the
/// result still round-trips, and set+unset on a pristine config restores
/// the default (proving no collateral damage to unrelated keys).
#[test]
fn edit_matrix_set_roundtrip_unset() {
    let (kitchen_sink, _) = parse("kitchen-sink.toml");

    for (key, value) in EDIT_MATRIX {
        // `concurrency.solves` defaults to the local CPU count, so a fixed
        // literal could equal the default on some machine (macOS CI runners
        // have 3 CPUs) and make `set` a no-op. Derive a value guaranteed to
        // differ from the default instead.
        let value: String = if *key == "concurrency.solves" {
            (Config::default().concurrency.solves + 1).to_string()
        } else {
            (*value).to_string()
        };

        // set on a fully populated config …
        let mut edited = kitchen_sink.clone();
        edited
            .set(key, Some(value.clone()))
            .unwrap_or_else(|e| panic!("set {key}={value} must succeed: {e}"));

        // … and the result still round-trips losslessly.
        let toml = edited.to_toml().unwrap();
        let (reloaded, unused) = Config::from_toml_str(&toml).unwrap();
        assert!(unused.is_empty(), "{key}: no unknown keys after edit");
        assert_eq!(edited, reloaded, "{key}: round trip after edit");

        // set + unset on a pristine config restores the default state,
        // proving the edit touched nothing else.
        let mut pristine = Config::default();
        pristine.set(key, Some(value.clone())).unwrap();
        assert_ne!(pristine, Config::default(), "{key}: set must change state");
        pristine.set(key, None).unwrap();
        assert_eq!(
            pristine,
            Config::default(),
            "{key}: unset must restore the default without collateral changes"
        );
    }
}

/// Unknown keys must be rejected by `set` (both set and unset direction).
#[test]
fn edit_rejects_unknown_keys() {
    let mut config = Config::default();
    assert!(
        config
            .set("definitely-a-typo", Some("1".to_string()))
            .is_err()
    );
    assert!(config.set("definitely-a-typo", None).is_err());
    assert!(
        config
            .set("concurrency.bogus", Some("1".to_string()))
            .is_err()
    );
}

/// Merge semantics per key family: scalars are replaced, mirrors extend,
/// per-channel tables merge recursively, s3 buckets accumulate. Snapshot
/// the merged result so semantic changes are reviewed consciously.
#[test]
fn merge_semantics() {
    let merged = Config::load_from_files([
        fixture_path("kitchen-sink.toml"),
        fixture_path("override-layer.toml"),
    ])
    .unwrap();

    // Spot-check the contract before snapshotting:
    // scalars/lists: later layer replaces.
    assert_eq!(
        merged.default_channels.as_ref().map(|c| c[0].to_string()),
        Some("robostack".to_string())
    );
    assert_eq!(merged.tls_no_verify, Some(true));
    // maps: later layer extends.
    assert_eq!(merged.mirrors.len(), 2);
    assert_eq!(merged.s3_options.0.len(), 2);
    // nested tables merge field-wise; unset fields keep the lower layer.
    assert_eq!(merged.repodata_config.default.disable_bzip2, Some(false));
    assert_eq!(merged.repodata_config.default.disable_zstd, Some(false));
    let prefix_dev = url::Url::parse("https://prefix.dev").unwrap();
    let per_channel = &merged.repodata_config.per_channel[&prefix_dev];
    assert_eq!(per_channel.disable_sharded, Some(true)); // from layer 1
    assert_eq!(per_channel.disable_zstd, Some(true)); // from layer 2
    // concurrency: explicitly set values win over the lower layer.
    assert_eq!(merged.concurrency.solves, 9);
    assert_eq!(merged.concurrency.downloads, 12);
    // provenance is recorded in order.
    assert_eq!(merged.loaded_from.len(), 2);

    insta::assert_snapshot!(
        "merge__kitchen_sink_plus_override",
        merged.to_toml().unwrap()
    );
}
