//! End-to-end integration tests for the shared configuration layer:
//! `ConfigLayer`/`ConfigLocation`, `load_from_locations`,
//! `from_toml_str_shared`, the layered search paths and the tracing
//! warnings emitted for ignored keys.

use std::ffi::OsStr;
use std::path::PathBuf;
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use rattler_config::config::{Config, ConfigBase, MergeError};
use rattler_config::locations::{ConfigLayer, ConfigLocation, config_search_paths};
use serde::{Deserialize, Serialize};
use tempfile::TempDir;
use tracing::field::{Field, Visit};
use tracing::{Event, Level, Metadata, Subscriber, span};
use url::Url;

/// A tool-specific extension, mirroring what pixi/rattler-build would do.
#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct ToolExt {
    #[serde(default)]
    custom_field: Option<String>,
    #[serde(default)]
    numeric_field: Option<u32>,
}

impl Config for ToolExt {
    fn merge_config(self, other: &Self) -> Result<Self, MergeError> {
        Ok(Self {
            custom_field: other.custom_field.clone().or(self.custom_field),
            numeric_field: other.numeric_field.or(self.numeric_field),
        })
    }
}

type ToolConfig = ConfigBase<ToolExt>;

// ---------------------------------------------------------------------------
// Warning capture: a minimal tracing subscriber recording WARN messages.
// ---------------------------------------------------------------------------

struct RecordingSubscriber {
    warnings: Arc<Mutex<Vec<String>>>,
    next_id: AtomicU64,
}

struct MessageVisitor(Option<String>);

impl Visit for MessageVisitor {
    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        if field.name() == "message" {
            self.0 = Some(format!("{value:?}"));
        }
    }
}

impl Subscriber for RecordingSubscriber {
    fn enabled(&self, _metadata: &Metadata<'_>) -> bool {
        true
    }
    fn new_span(&self, _span: &span::Attributes<'_>) -> span::Id {
        span::Id::from_u64(self.next_id.fetch_add(1, Ordering::Relaxed) + 1)
    }
    fn record(&self, _span: &span::Id, _values: &span::Record<'_>) {}
    fn record_follows_from(&self, _span: &span::Id, _follows: &span::Id) {}
    fn event(&self, event: &Event<'_>) {
        if *event.metadata().level() != Level::WARN {
            return;
        }
        let mut visitor = MessageVisitor(None);
        event.record(&mut visitor);
        if let Some(message) = visitor.0 {
            self.warnings.lock().unwrap().push(message);
        }
    }
    fn enter(&self, _span: &span::Id) {}
    fn exit(&self, _span: &span::Id) {}
}

/// Run `f` with a thread-local recording subscriber and return the result
/// together with all WARN-level messages emitted during the call.
fn capture_warnings<R>(f: impl FnOnce() -> R) -> (R, Vec<String>) {
    let warnings = Arc::new(Mutex::new(Vec::new()));
    let subscriber = RecordingSubscriber {
        warnings: Arc::clone(&warnings),
        next_id: AtomicU64::new(0),
    };
    let result = tracing::subscriber::with_default(subscriber, f);
    let warnings = warnings.lock().unwrap().clone();
    (result, warnings)
}

fn write_file(dir: &TempDir, name: &str, content: &str) -> PathBuf {
    let path = dir.path().join(name);
    std::fs::write(&path, content).unwrap();
    path
}

const SHARED_WARNING_MARKER: &str = "not a key shared by all rattler-based tools";
const TOOL_WARNING_MARKER: &str = "Ignoring unknown configuration key";

// ---------------------------------------------------------------------------
// 1. Shared-layer file: only common keys honored, everything else ignored
//    with the shared warning.
// ---------------------------------------------------------------------------

#[test]
fn shared_layer_honors_common_keys_only_and_warns() {
    let dir = TempDir::new().unwrap();
    let shared = write_file(
        &dir,
        "shared.toml",
        r#"
        default-channels = ["conda-forge"]
        tls-no-verify = true
        custom_field = "an extension key the tool itself understands"
        definitely-a-typo = 1
        "#,
    );

    let (result, warnings) = capture_warnings(|| {
        ToolConfig::load_from_locations([ConfigLocation {
            path: shared.clone(),
            layer: ConfigLayer::Shared,
        }])
    });
    let config = result.unwrap();

    // Common keys are honored.
    assert_eq!(
        config.default_channels,
        Some(vec!["conda-forge".parse().unwrap()])
    );
    assert_eq!(config.tls_no_verify, Some(true));

    // The extension stays at its default even though the tool knows the key.
    assert_eq!(config.extensions, ToolExt::default());

    // Both ignored keys warn with the shared-layer message.
    let shared_warnings: Vec<&String> = warnings
        .iter()
        .filter(|w| w.contains(SHARED_WARNING_MARKER))
        .collect();
    assert!(
        shared_warnings.iter().any(|w| w.contains("`custom_field`")),
        "expected a shared-layer warning for custom_field, got: {warnings:?}"
    );
    assert!(
        shared_warnings
            .iter()
            .any(|w| w.contains("`definitely-a-typo`")),
        "expected a shared-layer warning for definitely-a-typo, got: {warnings:?}"
    );
    // The warnings name the offending file.
    assert!(
        shared_warnings
            .iter()
            .all(|w| w.contains(shared.display().to_string().as_str())),
        "shared warnings must name the file, got: {warnings:?}"
    );
    // No tool-layer warning text for a shared file.
    assert!(
        warnings.iter().all(|w| !w.contains(TOOL_WARNING_MARKER)),
        "shared file must not use the tool-layer warning, got: {warnings:?}"
    );

    assert_eq!(config.loaded_from, vec![shared]);
}

// ---------------------------------------------------------------------------
// 2. Tool-layer file: common + extension keys accepted, unknown keys warn
//    with the tool message.
// ---------------------------------------------------------------------------

#[test]
fn tool_layer_accepts_extension_keys_and_warns_on_unknown() {
    let dir = TempDir::new().unwrap();
    let tool = write_file(
        &dir,
        "tool.toml",
        r#"
        default-channels = ["bioconda"]
        custom_field = "consumed"
        numeric_field = 42
        definitely-a-typo = 1
        "#,
    );

    let (result, warnings) = capture_warnings(|| {
        ToolConfig::load_from_locations([ConfigLocation {
            path: tool.clone(),
            layer: ConfigLayer::Tool,
        }])
    });
    let config = result.unwrap();

    assert_eq!(
        config.default_channels,
        Some(vec!["bioconda".parse().unwrap()])
    );
    assert_eq!(config.extensions.custom_field.as_deref(), Some("consumed"));
    assert_eq!(config.extensions.numeric_field, Some(42));

    // Exactly the typo warns, with the tool-layer message.
    assert!(
        warnings
            .iter()
            .any(|w| w.contains(TOOL_WARNING_MARKER) && w.contains("`definitely-a-typo`")),
        "expected a tool-layer warning for definitely-a-typo, got: {warnings:?}"
    );
    // Extension keys must not be warned about in a tool file.
    assert!(
        warnings.iter().all(|w| !w.contains("custom_field")),
        "tool file must not warn about its own extension keys, got: {warnings:?}"
    );
    assert!(
        warnings.iter().all(|w| !w.contains(SHARED_WARNING_MARKER)),
        "tool file must not use the shared-layer warning, got: {warnings:?}"
    );
}

// ---------------------------------------------------------------------------
// 6. Same file content: shared parse warns about the extension key, tool
//    parse does not.
// ---------------------------------------------------------------------------

#[test]
fn same_content_warns_as_shared_but_not_as_tool() {
    let dir = TempDir::new().unwrap();
    let content = r#"
        default-channels = ["conda-forge"]
        custom_field = "extension key"
        "#;
    let path = write_file(&dir, "config.toml", content);

    let (shared_result, shared_warnings) = capture_warnings(|| {
        ToolConfig::load_from_locations([ConfigLocation {
            path: path.clone(),
            layer: ConfigLayer::Shared,
        }])
    });
    shared_result.unwrap();
    assert!(
        shared_warnings
            .iter()
            .any(|w| w.contains(SHARED_WARNING_MARKER) && w.contains("`custom_field`")),
        "shared parse must warn about custom_field, got: {shared_warnings:?}"
    );

    let (tool_result, tool_warnings) = capture_warnings(|| {
        ToolConfig::load_from_locations([ConfigLocation {
            path: path.clone(),
            layer: ConfigLayer::Tool,
        }])
    });
    tool_result.unwrap();
    assert!(
        tool_warnings.is_empty(),
        "tool parse of the same content must not warn, got: {tool_warnings:?}"
    );
}

// ---------------------------------------------------------------------------
// 3. Realistic 4-file stack: system shared, system tool, user shared, user
//    tool. Later files win per key, earlier-only keys survive, maps merge
//    additively, loaded_from records all files in order, and extension keys
//    in shared files never leak into the extension.
// ---------------------------------------------------------------------------

#[test]
fn four_file_stack_merges_with_correct_precedence() {
    let dir = TempDir::new().unwrap();
    let mirror_upstream_1 = "https://conda.anaconda.org/one/";
    let mirror_upstream_2 = "https://conda.anaconda.org/two/";
    let mirror_upstream_3 = "https://conda.anaconda.org/three/";

    let system_shared = write_file(
        &dir,
        "system_shared.toml",
        &format!(
            r#"
            default-channels = ["from-system-shared"]
            tls-no-verify = true
            custom_field = "from-system-shared"

            [mirrors]
            "{mirror_upstream_1}" = ["https://mirror.example/one-old/"]
            "#
        ),
    );
    let system_tool = write_file(
        &dir,
        "system_tool.toml",
        &format!(
            r#"
            default-channels = ["from-system-tool"]
            authentication-override-file = "/etc/auth.json"
            custom_field = "from-system-tool"

            [mirrors]
            "{mirror_upstream_2}" = ["https://mirror.example/two/"]
            "#
        ),
    );
    let user_shared = write_file(
        &dir,
        "user_shared.toml",
        &format!(
            r#"
            default-channels = ["from-user-shared"]
            allow-hard-links = false
            custom_field = "from-user-shared"

            [mirrors]
            "{mirror_upstream_1}" = ["https://mirror.example/one-new/"]
            "#
        ),
    );
    let user_tool = write_file(
        &dir,
        "user_tool.toml",
        &format!(
            r#"
            default-channels = ["from-user-tool"]
            numeric_field = 7

            [mirrors]
            "{mirror_upstream_3}" = ["https://mirror.example/three/"]
            "#
        ),
    );

    let (result, warnings) = capture_warnings(|| {
        ToolConfig::load_from_locations([
            ConfigLocation {
                path: system_shared.clone(),
                layer: ConfigLayer::Shared,
            },
            ConfigLocation {
                path: system_tool.clone(),
                layer: ConfigLayer::Tool,
            },
            ConfigLocation {
                path: user_shared.clone(),
                layer: ConfigLayer::Shared,
            },
            ConfigLocation {
                path: user_tool.clone(),
                layer: ConfigLayer::Tool,
            },
        ])
    });
    let config = result.unwrap();

    // Later files win per key.
    assert_eq!(
        config.default_channels,
        Some(vec!["from-user-tool".parse().unwrap()])
    );
    // Keys set only in earlier files survive.
    assert_eq!(config.tls_no_verify, Some(true), "from system shared");
    assert_eq!(
        config.authentication_override_file,
        Some(PathBuf::from("/etc/auth.json")),
        "from system tool"
    );
    assert_eq!(config.allow_hard_links, Some(false), "from user shared");

    // Mirrors merge additively across all four files; the later shared file
    // overrides the earlier one per upstream URL.
    let mirrors = &config.mirrors;
    assert_eq!(
        mirrors.len(),
        3,
        "mirrors must merge additively: {mirrors:?}"
    );
    assert_eq!(
        mirrors[&Url::parse(mirror_upstream_1).unwrap()],
        vec![Url::parse("https://mirror.example/one-new/").unwrap()],
        "later shared file must override the earlier one per key"
    );
    assert_eq!(
        mirrors[&Url::parse(mirror_upstream_2).unwrap()],
        vec![Url::parse("https://mirror.example/two/").unwrap()]
    );
    assert_eq!(
        mirrors[&Url::parse(mirror_upstream_3).unwrap()],
        vec![Url::parse("https://mirror.example/three/").unwrap()]
    );

    // Extension keys come only from tool files. `custom_field` is set in
    // both shared files (later than the system tool file!) but must keep
    // the system tool value; `numeric_field` comes from the user tool file.
    assert_eq!(
        config.extensions.custom_field.as_deref(),
        Some("from-system-tool"),
        "extension keys in shared files must not leak into the extension"
    );
    assert_eq!(config.extensions.numeric_field, Some(7));

    // loaded_from records all files in load order.
    assert_eq!(
        config.loaded_from,
        vec![system_shared, system_tool, user_shared, user_tool]
    );

    // Both shared files warned about their extension key; the tool files
    // did not warn at all.
    let shared_warning_count = warnings
        .iter()
        .filter(|w| w.contains(SHARED_WARNING_MARKER) && w.contains("`custom_field`"))
        .count();
    assert_eq!(
        shared_warning_count, 2,
        "each shared file must warn about custom_field, got: {warnings:?}"
    );
    assert!(
        warnings.iter().all(|w| !w.contains(TOOL_WARNING_MARKER)),
        "no tool-layer warnings expected, got: {warnings:?}"
    );
}

// ---------------------------------------------------------------------------
// 4. `load_from_files` still parses everything as the tool layer.
// ---------------------------------------------------------------------------

#[test]
fn load_from_files_parses_all_files_as_tool_layer() {
    let dir = TempDir::new().unwrap();
    let first = write_file(
        &dir,
        "first.toml",
        r#"
        custom_field = "from-first"
        numeric_field = 1
        "#,
    );
    let second = write_file(
        &dir,
        "second.toml",
        r#"
        custom_field = "from-second"
        "#,
    );

    let (result, warnings) =
        capture_warnings(|| ToolConfig::load_from_files([first.clone(), second.clone()]));
    let config = result.unwrap();

    assert_eq!(
        config.extensions.custom_field.as_deref(),
        Some("from-second")
    );
    assert_eq!(config.extensions.numeric_field, Some(1));
    assert!(
        warnings.is_empty(),
        "extension keys must be consumed without warnings, got: {warnings:?}"
    );
    assert_eq!(config.loaded_from, vec![first, second]);
}

// ---------------------------------------------------------------------------
// Shared files with a malformed common key still fail to parse.
// ---------------------------------------------------------------------------

#[test]
fn shared_layer_still_rejects_malformed_common_values() {
    let dir = TempDir::new().unwrap();
    let shared = write_file(&dir, "bad.toml", "tls-no-verify = \"not-a-bool\"\n");

    let result = ToolConfig::load_from_locations([ConfigLocation {
        path: shared,
        layer: ConfigLayer::Shared,
    }]);
    assert!(
        result.is_err(),
        "malformed common value in a shared file must be an error"
    );
}

// ---------------------------------------------------------------------------
// 5. `config_search_paths` interleaving, layer tags and RATTLER_HOME
//    behavior. Environment-mutating assertions run in a child process (this
//    same test binary, filtered to one probe test) so parallel tests in
//    this binary are never affected.
// ---------------------------------------------------------------------------

const PROBE_ENV: &str = "SHARED_LAYER_ENV_PROBE";

fn run_probe(probe_name: &str, marker: &str, envs: &[(&str, Option<&OsStr>)]) {
    let exe = std::env::current_exe().unwrap();
    let mut command = Command::new(exe);
    command.args(["--exact", probe_name, "--nocapture"]);
    // Start from a known state for every variable the probes look at.
    for var in ["RATTLER_HOME", "XDG_CONFIG_HOME", "HOME", "SOME_TOOL_HOME"] {
        command.env_remove(var);
    }
    for (key, value) in envs {
        match value {
            Some(value) => command.env(key, value),
            None => command.env_remove(key),
        };
    }
    command.env(PROBE_ENV, marker);
    let output = command.output().unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "probe {probe_name} failed\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    // Guard against `--exact` silently matching zero tests (which exits 0).
    assert!(
        stdout.contains("PROBE-DONE"),
        "probe {probe_name} did not run\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
}

/// Probe body: with `RATTLER_HOME` set, `$RATTLER_HOME/config.toml` appears as
/// a Shared location and the full interleaving is
/// system-shared, system-tool, user-shared(s), user-tool(s).
#[test]
fn env_probe_search_paths_with_rattler_home() {
    if std::env::var(PROBE_ENV).as_deref() != Ok("with-rattler-home") {
        return;
    }
    let rattler_home = PathBuf::from(std::env::var("RATTLER_HOME").unwrap());

    let locations = config_search_paths("some-tool");

    #[cfg(target_os = "linux")]
    {
        let xdg = PathBuf::from(std::env::var("XDG_CONFIG_HOME").unwrap());
        let home = PathBuf::from(std::env::var("HOME").unwrap());
        let expected = vec![
            ConfigLocation {
                path: PathBuf::from("/etc/rattler/config.toml"),
                layer: ConfigLayer::Shared,
            },
            ConfigLocation {
                path: PathBuf::from("/etc/some-tool/config.toml"),
                layer: ConfigLayer::Tool,
            },
            ConfigLocation {
                path: xdg.join("rattler").join("config.toml"),
                layer: ConfigLayer::Shared,
            },
            ConfigLocation {
                path: rattler_home.join("config.toml"),
                layer: ConfigLayer::Shared,
            },
            ConfigLocation {
                path: xdg.join("some-tool").join("config.toml"),
                layer: ConfigLayer::Tool,
            },
            ConfigLocation {
                path: home.join(".some-tool").join("config.toml"),
                layer: ConfigLayer::Tool,
            },
        ];
        assert_eq!(locations, expected);
    }
    #[cfg(not(target_os = "linux"))]
    {
        let rattler_home_location = locations
            .iter()
            .find(|l| l.path == rattler_home.join("config.toml"))
            .expect("RATTLER_HOME/config.toml must be a search path");
        assert_eq!(rattler_home_location.layer, ConfigLayer::Shared);
    }
    println!("PROBE-DONE");
}

/// Probe body: without `RATTLER_HOME` there is no `~/.rattler` fallback, and
/// the interleaving is system-shared, system-tool, user-shared, user-tool.
#[test]
fn env_probe_search_paths_without_rattler_home() {
    if std::env::var(PROBE_ENV).as_deref() != Ok("without-rattler-home") {
        return;
    }
    let home = PathBuf::from(std::env::var("HOME").unwrap());

    let locations = config_search_paths("some-tool");

    // No ~/.rattler path may appear anywhere.
    let dot_rattler = home.join(".rattler").join("config.toml");
    assert!(
        locations.iter().all(|l| l.path != dot_rattler),
        "shared layer must have no ~/.rattler fallback, got: {locations:?}"
    );
    assert!(
        locations
            .iter()
            .all(|l| !l.path.to_string_lossy().contains(".rattler")),
        "no .rattler dotdir expected, got: {locations:?}"
    );

    #[cfg(target_os = "linux")]
    {
        let xdg = PathBuf::from(std::env::var("XDG_CONFIG_HOME").unwrap());
        let expected = vec![
            ConfigLocation {
                path: PathBuf::from("/etc/rattler/config.toml"),
                layer: ConfigLayer::Shared,
            },
            ConfigLocation {
                path: PathBuf::from("/etc/some-tool/config.toml"),
                layer: ConfigLayer::Tool,
            },
            ConfigLocation {
                path: xdg.join("rattler").join("config.toml"),
                layer: ConfigLayer::Shared,
            },
            ConfigLocation {
                path: xdg.join("some-tool").join("config.toml"),
                layer: ConfigLayer::Tool,
            },
            ConfigLocation {
                path: home.join(".some-tool").join("config.toml"),
                layer: ConfigLayer::Tool,
            },
        ];
        assert_eq!(locations, expected);
    }
    println!("PROBE-DONE");
}

#[test]
fn search_paths_respect_rattler_home() {
    let rattler_home = TempDir::new().unwrap();
    let xdg = TempDir::new().unwrap();
    let home = TempDir::new().unwrap();
    run_probe(
        "env_probe_search_paths_with_rattler_home",
        "with-rattler-home",
        &[
            ("RATTLER_HOME", Some(rattler_home.path().as_os_str())),
            ("XDG_CONFIG_HOME", Some(xdg.path().as_os_str())),
            ("HOME", Some(home.path().as_os_str())),
        ],
    );
}

#[test]
fn search_paths_without_rattler_home_have_no_dotdir() {
    let xdg = TempDir::new().unwrap();
    let home = TempDir::new().unwrap();
    run_probe(
        "env_probe_search_paths_without_rattler_home",
        "without-rattler-home",
        &[
            ("RATTLER_HOME", None),
            ("XDG_CONFIG_HOME", Some(xdg.path().as_os_str())),
            ("HOME", Some(home.path().as_os_str())),
        ],
    );
}

/// Probe body: when `RATTLER_HOME` and `SOME_TOOL_HOME` point at the same
/// directory, the colliding path is deduplicated to a single entry. A path
/// that appears in both layers is always parsed as a tool file, whichever
/// occurrence survives the dedup.
#[test]
fn env_probe_search_paths_dedup_on_layer_collision() {
    if std::env::var(PROBE_ENV).as_deref() != Ok("layer-collision") {
        return;
    }
    let shared_home = PathBuf::from(std::env::var("RATTLER_HOME").unwrap());
    let colliding = shared_home.join("config.toml");

    let locations = config_search_paths("some-tool");

    let matches: Vec<&ConfigLocation> = locations.iter().filter(|l| l.path == colliding).collect();
    assert_eq!(
        matches.len(),
        1,
        "colliding path must be deduplicated to one entry, got: {locations:?}"
    );
    assert_eq!(
        matches[0].layer,
        ConfigLayer::Tool,
        "dedup must keep the highest-precedence occurrence (the tool layer)"
    );
    println!("PROBE-DONE");
}

#[test]
fn search_paths_dedup_collision_between_layers() {
    let shared_and_tool_home = TempDir::new().unwrap();
    let xdg = TempDir::new().unwrap();
    let home = TempDir::new().unwrap();
    run_probe(
        "env_probe_search_paths_dedup_on_layer_collision",
        "layer-collision",
        &[
            (
                "RATTLER_HOME",
                Some(shared_and_tool_home.path().as_os_str()),
            ),
            (
                "SOME_TOOL_HOME",
                Some(shared_and_tool_home.path().as_os_str()),
            ),
            ("XDG_CONFIG_HOME", Some(xdg.path().as_os_str())),
            ("HOME", Some(home.path().as_os_str())),
        ],
    );
}

/// Probe body: the reverse collision direction. `RATTLER_HOME` points at
/// the tool's *system* directory, so the shared occurrence of the colliding
/// path comes *after* the tool occurrence and survives the dedup. The entry
/// must still be parsed as a tool file: the tool's own system config
/// legitimately contains extension keys, and parsing it as shared would
/// silently drop them.
#[cfg(not(target_os = "windows"))]
#[test]
fn env_probe_search_paths_dedup_reverse_layer_collision() {
    if std::env::var(PROBE_ENV).as_deref() != Ok("reverse-layer-collision") {
        return;
    }
    let colliding = PathBuf::from("/etc/some-tool/config.toml");

    let locations = config_search_paths("some-tool");

    let matches: Vec<&ConfigLocation> = locations.iter().filter(|l| l.path == colliding).collect();
    assert_eq!(
        matches.len(),
        1,
        "colliding path must be deduplicated to one entry, got: {locations:?}"
    );
    assert_eq!(
        matches[0].layer,
        ConfigLayer::Tool,
        "a path in both layers must be parsed as a tool file"
    );
    println!("PROBE-DONE");
}

#[cfg(not(target_os = "windows"))]
#[test]
fn search_paths_dedup_reverse_collision_keeps_tool_layer() {
    let xdg = TempDir::new().unwrap();
    let home = TempDir::new().unwrap();
    run_probe(
        "env_probe_search_paths_dedup_reverse_layer_collision",
        "reverse-layer-collision",
        &[
            ("RATTLER_HOME", Some(OsStr::new("/etc/some-tool"))),
            ("XDG_CONFIG_HOME", Some(xdg.path().as_os_str())),
            ("HOME", Some(home.path().as_os_str())),
        ],
    );
}

// ---------------------------------------------------------------------------
// End-to-end through the default locations: a real 4-file stack in
// RATTLER_HOME / tool home / XDG dirs, loaded via
// `load_from_default_locations` in a child process with a controlled
// environment.
// ---------------------------------------------------------------------------

/// Probe body: build the user-level part of the stack on disk and load it
/// through `load_from_default_locations`.
///
/// Unix only: on Windows `dirs::config_dir` uses the known-folder API and
/// ignores `XDG_CONFIG_HOME`, so the files written below would never be
/// found.
#[cfg(unix)]
#[test]
fn env_probe_load_from_default_locations() {
    if std::env::var(PROBE_ENV).as_deref() != Ok("default-locations") {
        return;
    }
    let xdg = PathBuf::from(std::env::var("XDG_CONFIG_HOME").unwrap());
    let rattler_home = PathBuf::from(std::env::var("RATTLER_HOME").unwrap());

    // user shared (XDG): common key + extension key that must be ignored.
    let xdg_shared_dir = xdg.join("rattler");
    std::fs::create_dir_all(&xdg_shared_dir).unwrap();
    std::fs::write(
        xdg_shared_dir.join("config.toml"),
        r#"
        default-channels = ["from-xdg-shared"]
        tls-no-verify = true
        custom_field = "leaked-from-xdg-shared"
        "#,
    )
    .unwrap();

    // user shared (RATTLER_HOME): higher precedence than the XDG shared file.
    std::fs::write(
        rattler_home.join("config.toml"),
        r#"
        default-channels = ["from-rattler-home"]
        custom_field = "leaked-from-rattler-home"
        "#,
    )
    .unwrap();

    // user tool (XDG): extension key must be consumed.
    let xdg_tool_dir = xdg.join("some-tool");
    std::fs::create_dir_all(&xdg_tool_dir).unwrap();
    std::fs::write(
        xdg_tool_dir.join("config.toml"),
        r#"
        default-channels = ["from-xdg-tool"]
        custom_field = "from-xdg-tool"
        "#,
    )
    .unwrap();

    let config = ToolConfig::load_from_default_locations("some-tool").unwrap();

    // The tool file has the highest precedence among the files we created.
    assert_eq!(
        config.default_channels,
        Some(vec!["from-xdg-tool".parse().unwrap()]),
        "user tool file must win"
    );
    // A common key set only in the lowest shared file survives.
    assert_eq!(config.tls_no_verify, Some(true));
    // Extension keys in shared files never reach the extension.
    assert_eq!(
        config.extensions.custom_field.as_deref(),
        Some("from-xdg-tool"),
        "extension value must come from the tool file only"
    );
    // All three files we created were recorded, in precedence order. The
    // comparison ignores files outside the controlled environment (a real
    // `/etc/rattler/config.toml` or `/etc/some-tool/config.toml` may exist
    // on the machine running the tests and loads with lower precedence).
    let ours: Vec<PathBuf> = config
        .loaded_from
        .iter()
        .filter(|path| !path.starts_with("/etc"))
        .cloned()
        .collect();
    assert_eq!(
        ours,
        vec![
            xdg_shared_dir.join("config.toml"),
            rattler_home.join("config.toml"),
            xdg_tool_dir.join("config.toml"),
        ]
    );
    println!("PROBE-DONE");
}

#[cfg(unix)]
#[test]
fn load_from_default_locations_layers_shared_and_tool() {
    let rattler_home = TempDir::new().unwrap();
    let xdg = TempDir::new().unwrap();
    let home = TempDir::new().unwrap();
    run_probe(
        "env_probe_load_from_default_locations",
        "default-locations",
        &[
            ("RATTLER_HOME", Some(rattler_home.path().as_os_str())),
            ("XDG_CONFIG_HOME", Some(xdg.path().as_os_str())),
            ("HOME", Some(home.path().as_os_str())),
        ],
    );
}
