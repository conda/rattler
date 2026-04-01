//! Provides reading and writing of the `history` file found in conda environments.
//!
//! The history file lives at `<prefix>/conda-meta/history` and records every
//! install/remove/update transaction that has been performed on an environment.
//! An example of this file format is shown below:
//!
//! ```text
//! ==> 2026-04-01 16:26:51 <==
//! # cmd: /home/rattler/miniconda/bin/conda create -n history-test xz ca-certificates --yes
//! # conda version: 26.1.1
//! +conda-forge/noarch::ca-certificates-2026.2.25-hbd8a1cb_0
//! +conda-forge/osx-arm64::liblzma-5.8.2-h8088a28_0
//! +conda-forge/osx-arm64::liblzma-devel-5.8.2-h8088a28_0
//! +conda-forge/osx-arm64::xz-5.8.2-hd0f0c4f_0
//! +conda-forge/osx-arm64::xz-gpl-tools-5.8.2-hd0f0c4f_0
//! +conda-forge/osx-arm64::xz-tools-5.8.2-h8088a28_0
//! # update specs: ['ca-certificates', 'xz']
//! ==> 2026-04-01 16:27:37 <==
//! # cmd: /home/rattler/miniconda/bin/conda remove -n history-test ca-certificates --yes
//! # conda version: 26.1.1
//! -conda-forge/noarch::ca-certificates-2026.2.25-hbd8a1cb_0
//! # remove specs: ['ca-certificates']
//! ```
//!
//! Each transaction begins with a header that includes the date followed by some
//! metadata about the transaction including the current command and conda client version.
//! The content of the transaction is a list of `MatchSpecs` prefaced with either a
//! `+` or `-` to indicate either an addition or removal action, respectively.
//! The transaction block ends with the "updated" or "remove" specs indicating what
//! the user wished to either add or remove.

#![deny(missing_docs)]

use std::{
    collections::{HashMap, HashSet},
    io::{self, BufWriter, Write},
    path::{Path, PathBuf},
    str::FromStr,
};

use chrono::NaiveDateTime;
use thiserror::Error;

// ── Compiled regexes ──────────────────────────────────────────────────────────
// Declared as module-level statics so their names document intent and so that
// `lazy_regex::lazy_regex!` (which produces a `static`) is clearly separated
// from runtime logic.

/// Matches the section-header line: `==> 2024-01-01 12:00:00 <==`
static SECTION_HEADER_RE: lazy_regex::Lazy<lazy_regex::Regex> =
    lazy_regex::lazy_regex!(r"^==>\s*(.+?)\s*<==\s*$");

/// Matches a `# cmd: <argv...>` comment line.
static CMD_RE: lazy_regex::Lazy<lazy_regex::Regex> =
    lazy_regex::lazy_regex!(r"^#\s*cmd:\s*(.+)$");

/// Matches a `# conda version: <ver>` comment line.
static CONDA_VERSION_RE: lazy_regex::Lazy<lazy_regex::Regex> =
    lazy_regex::lazy_regex!(r"^#\s*conda version:\s*(.+)$");

/// Matches `# <action> specs: <specs>` comment lines.
static SPECS_RE: lazy_regex::Lazy<lazy_regex::Regex> =
    lazy_regex::lazy_regex!(r"^#\s*(\w+)\s+specs:\s*(.*)$");

/// Matches a single quoted item inside a Python list literal.
static QUOTED_ITEM_RE: lazy_regex::Lazy<lazy_regex::Regex> =
    lazy_regex::lazy_regex!(r#"['"]([^'"]*)['"]\s*,?\s*"#);

/// Matches the start of a version-relation operator (`<`, `>`, `=`, `!`).
static VERSION_RELATION_RE: lazy_regex::Lazy<lazy_regex::Regex> =
    lazy_regex::lazy_regex!(r"^[<>=!]");

// ── Error type ────────────────────────────────────────────────────────────────

/// Errors that can occur while working with a conda history file.
#[derive(Debug, Error)]
pub enum HistoryError {
    /// An I/O error occurred while reading or writing the history file.
    #[error("I/O error: {0}")]
    Io(#[from] io::Error),

    /// The history file contains an entry that is neither a `+` add nor a `-`
    /// remove, which is invalid in a diff section.
    #[error("unexpected diff entry in history: {0}")]
    UnexpectedDiffEntry(String),

    /// A timestamp in the history file could not be parsed.
    #[error("could not parse timestamp '{timestamp}'")]
    InvalidTimestamp {
        /// The raw string that failed to parse.
        timestamp: String,
        /// The underlying parse error.
        #[source]
        source: chrono::format::ParseError,
    },
}

// ── HistoryAction ─────────────────────────────────────────────────────────────

/// The action type recorded in a history comment line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HistoryAction {
    /// Packages were installed (also covers `create`).
    Install,
    /// Packages were updated.
    Update,
    /// Packages were removed (also covers `uninstall`).
    Remove,
    /// Specs were pinned/neutered to prevent unintended upgrades.
    Neutered,
    /// An unrecognised action string found in the file.
    Other(String),
}

/// Infallible: unknown strings become [`HistoryAction::Other`].
impl FromStr for HistoryAction {
    type Err = std::convert::Infallible;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(match s {
            "install" | "create" => Self::Install,
            "update" => Self::Update,
            "remove" | "uninstall" => Self::Remove,
            "neutered" => Self::Neutered,
            other => Self::Other(other.to_owned()),
        })
    }
}

// ── HistoryCommentLine ────────────────────────────────────────────────────────

/// The specs payload carried by a `# <action> specs: …` comment line.
///
/// Grouping the action and its specs together avoids four parallel `Vec`
/// fields that would otherwise always have at most one populated.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SpecsComment {
    /// Specs that were installed or updated (`install`, `update`, `create`).
    Update(Vec<String>),
    /// Specs that were removed (`remove`, `uninstall`).
    Remove(Vec<String>),
    /// Specs that were neutered/pinned.
    Neutered(Vec<String>),
    /// Specs for an unrecognised action — preserved verbatim.
    Other {
        /// The unrecognised action name as it appeared in the file.
        action: String,
        /// The raw spec strings.
        specs: Vec<String>,
    },
}

impl SpecsComment {
    /// The raw spec strings, regardless of action kind.
    pub fn specs(&self) -> &[String] {
        match self {
            Self::Update(s) | Self::Remove(s) | Self::Neutered(s) => s,
            Self::Other { specs, .. } => specs,
        }
    }
}

/// A single parsed comment line from a history entry.
///
/// Each comment line carries *at most one* of: a `cmd`, a `conda_version`,
/// or a `specs` payload.
#[derive(Debug, Clone)]
pub enum HistoryCommentLine {
    /// `# cmd: <argv…>`
    Cmd(Vec<String>),
    /// `# conda version: <ver>`
    CondaVersion(String),
    /// `# <action> specs: <specs>`
    Specs(SpecsComment),
    /// Any other comment line (preserved verbatim without the leading `#`).
    Other(String),
}

// ── HistoryEntry ──────────────────────────────────────────────────────────────

/// A single transaction entry in the history file.
///
/// Each entry corresponds to one conda operation and contains:
/// - a timestamp,
/// - a set of package distributions (either a full snapshot or a diff), and
/// - zero or more parsed comment lines.
#[derive(Debug, Clone)]
pub struct HistoryEntry {
    /// The date/time at which this transaction was recorded.
    ///
    /// Use [`NaiveDateTime::format`] to reproduce the original string, e.g.
    /// `entry.date.format("%Y-%m-%d %H:%M:%S")`.
    pub date: NaiveDateTime,

    /// Package distributions included in this entry.
    ///
    /// In a *diff* entry each string is prefixed with `+` (added) or `-`
    /// (removed).  In a *snapshot* entry the strings have no prefix.
    pub packages: HashSet<String>,

    /// Parsed comment lines associated with this entry.
    pub comments: Vec<HistoryCommentLine>,
}

impl HistoryEntry {
    /// Returns `true` if this entry represents a diff (i.e. at least one
    /// package string starts with `+` or `-`).
    pub fn is_diff(&self) -> bool {
        self.packages.iter().any(|s| s.starts_with(['+', '-']))
    }

    /// Returns the `cmd` argv from the first matching comment line, if any.
    pub fn cmd(&self) -> Option<&[String]> {
        self.comments.iter().find_map(|c| match c {
            HistoryCommentLine::Cmd(argv) => Some(argv.as_slice()),
            _ => None,
        })
    }

    /// Returns the conda version from the first matching comment line, if any.
    pub fn conda_version(&self) -> Option<&str> {
        self.comments.iter().find_map(|c| match c {
            HistoryCommentLine::CondaVersion(v) => Some(v.as_str()),
            _ => None,
        })
    }

    /// Returns an iterator over all [`SpecsComment`] lines in this entry.
    pub fn specs_comments(&self) -> impl Iterator<Item = &SpecsComment> {
        self.comments.iter().filter_map(|c| match c {
            HistoryCommentLine::Specs(s) => Some(s),
            _ => None,
        })
    }
}

// ── UserRequest ───────────────────────────────────────────────────────────────

/// A user-level request extracted from the history file.
///
/// This is a higher-level view of a [`HistoryEntry`] that focuses on *what the
/// user asked for* rather than the raw package diff.
#[derive(Debug, Clone)]
pub struct UserRequest {
    /// The date/time of the transaction.
    pub date: NaiveDateTime,

    /// The command-line invocation that triggered the transaction.
    pub cmd: Vec<String>,

    /// The conda version that performed this transaction, if recorded.
    pub conda_version: Option<String>,

    /// All spec payloads from the entry's comment lines.
    pub specs: Vec<SpecsComment>,

    /// Packages that were unlinked (removed from the environment).
    pub unlink_dists: Vec<String>,

    /// Packages that were linked (added to the environment).
    pub link_dists: Vec<String>,
}

impl UserRequest {
    /// Iterate over every update/install spec in this request.
    pub fn update_specs(&self) -> impl Iterator<Item = &str> {
        self.specs
            .iter()
            .flat_map(|s| match s {
                SpecsComment::Update(specs) => specs.as_slice(),
                _ => &[],
            })
            .map(String::as_str)
    }

    /// Iterate over every remove spec in this request.
    pub fn remove_specs(&self) -> impl Iterator<Item = &str> {
        self.specs
            .iter()
            .flat_map(|s| match s {
                SpecsComment::Remove(specs) => specs.as_slice(),
                _ => &[],
            })
            .map(String::as_str)
    }

    /// Iterate over every neutered/pinned spec in this request.
    pub fn neutered_specs(&self) -> impl Iterator<Item = &str> {
        self.specs
            .iter()
            .flat_map(|s| match s {
                SpecsComment::Neutered(specs) => specs.as_slice(),
                _ => &[],
            })
            .map(String::as_str)
    }
}

// ── History ───────────────────────────────────────────────────────────────────

/// Provides read/write access to the conda `history` file of an environment.
///
/// The history file lives at `<prefix>/conda-meta/history`.
///
/// # Example
///
/// ```no_run
/// use std::path::Path;
/// use rattler_history::History;
///
/// let history = History::new(Path::new("/opt/conda"));
/// let state = history.get_latest_state().unwrap();
/// println!("Current state has {} packages", state.len());
/// ```
pub struct History {
    /// The root prefix of the conda environment.
    pub prefix: PathBuf,

    /// The `conda-meta` directory inside the prefix.
    pub meta_dir: PathBuf,

    /// The full path to the history file.
    pub path: PathBuf,
}

impl History {
    /// Create a new [`History`] for the given environment `prefix`.
    ///
    /// This does **not** create the file on disk; call [`History::init`] first
    /// if you need to ensure it exists.
    pub fn new(prefix: &Path) -> Self {
        let meta_dir = prefix.join("conda-meta");
        let path = meta_dir.join("history");
        Self {
            prefix: prefix.to_path_buf(),
            meta_dir,
            path,
        }
    }

    /// Ensure the history file exists, creating an empty one if necessary.
    pub fn init(&self) -> Result<(), HistoryError> {
        if !self.meta_dir.exists() {
            std::fs::create_dir_all(&self.meta_dir)?;
        }
        if !self.path.exists() {
            std::fs::File::create(&self.path)?;
        }
        Ok(())
    }

    /// Returns `true` if the history file exists and is empty.
    pub fn is_empty(&self) -> Result<bool, HistoryError> {
        Ok(self.path.exists() && std::fs::metadata(&self.path)?.len() == 0)
    }

    // ── Parsing ───────────────────────────────────────────────────────────

    /// Parse the history file into a list of [`HistoryEntry`] values.
    ///
    /// Comment lines that appear *before* the first section header are
    /// silently ignored (matching conda's own behaviour).
    pub fn parse(&self) -> Result<Vec<HistoryEntry>, HistoryError> {
        if !self.path.exists() {
            return Ok(Vec::new());
        }
        let content = std::fs::read_to_string(&self.path)?;
        parse_str(&content)
    }

    // ── State reconstruction ──────────────────────────────────────────────

    /// Reconstruct the sequence of full environment states from the history.
    ///
    /// Each returned item is a `(date, packages)` pair where `packages` is the
    /// complete set of distribution strings installed at that point in time.
    pub fn construct_states(
        &self,
    ) -> Result<Vec<(NaiveDateTime, HashSet<String>)>, HistoryError> {
        let entries = self.parse()?;
        let mut current = HashSet::<String>::new();

        entries
            .into_iter()
            .map(|entry| {
                if entry.is_diff() {
                    for pkg in &entry.packages {
                        if let Some(dist) = pkg.strip_prefix('+') {
                            current.insert(dist.to_owned());
                        } else if let Some(dist) = pkg.strip_prefix('-') {
                            current.remove(dist);
                        } else {
                            return Err(HistoryError::UnexpectedDiffEntry(pkg.clone()));
                        }
                    }
                } else {
                    current = entry.packages;
                    return Ok((entry.date, current.clone()));
                }
                Ok((entry.date, current.clone()))
            })
            .collect()
    }

    /// Return the environment state at the given revision index.
    ///
    /// Revisions are 0-based and ordered oldest-first. Returns an empty set
    /// if the history is empty or the revision is out of range.
    pub fn get_state_at(&self, rev: usize) -> Result<HashSet<String>, HistoryError> {
        Ok(self
            .construct_states()?
            .into_iter()
            .nth(rev)
            .map(|(_, pkgs)| pkgs)
            .unwrap_or_default())
    }

    /// Return the most recent environment state.
    ///
    /// Returns an empty set if the history file is empty or does not exist.
    pub fn get_latest_state(&self) -> Result<HashSet<String>, HistoryError> {
        Ok(self
            .construct_states()?
            .into_iter()
            .last()
            .map(|(_, pkgs)| pkgs)
            .unwrap_or_default())
    }

    /// Return a list of user-level requests extracted from the history.
    ///
    /// Only entries that contain a `# cmd:` line are included, matching
    /// conda's own `get_user_requests` behaviour.
    pub fn get_user_requests(&self) -> Result<Vec<UserRequest>, HistoryError> {
        self.parse()?
            .into_iter()
            .filter_map(|entry| {
                let cmd = entry.cmd()?.to_vec();
                let conda_version = entry.conda_version().map(str::to_owned);
                let specs: Vec<SpecsComment> = entry.specs_comments().cloned().collect();

                let (unlink_dists, link_dists) = if entry.is_diff() {
                    let mut removed: Vec<String> = entry
                        .packages
                        .iter()
                        .filter_map(|p| p.strip_prefix('-').map(str::to_owned))
                        .collect();
                    let mut added: Vec<String> = entry
                        .packages
                        .iter()
                        .filter_map(|p| p.strip_prefix('+').map(str::to_owned))
                        .collect();
                    removed.sort_unstable();
                    added.sort_unstable();
                    (removed, added)
                } else {
                    let mut snapshot: Vec<String> = entry.packages.into_iter().collect();
                    snapshot.sort_unstable();
                    (Vec::new(), snapshot)
                };

                Some(Ok(UserRequest {
                    date: entry.date,
                    cmd,
                    conda_version,
                    specs,
                    unlink_dists,
                    link_dists,
                }))
            })
            .collect()
    }

    /// Return a map from package name to the most recently requested spec.
    ///
    /// The map reflects the *current* desired state based on all install,
    /// update, remove and neutering operations recorded in the history.
    /// Packages that have been removed are excluded, as are packages whose
    /// name is not present in the provided `installed` set.
    pub fn get_requested_specs_map(
        &self,
        installed: &HashSet<String>,
    ) -> Result<HashMap<String, String>, HistoryError> {
        let mut spec_map = HashMap::<String, String>::new();

        for request in self.get_user_requests()? {
            for spec in request.remove_specs() {
                if let Some(name) = package_name_from_spec(spec) {
                    spec_map.remove(name);
                }
            }
            for spec in request.update_specs().chain(request.neutered_specs()) {
                if let Some(name) = package_name_from_spec(spec) {
                    spec_map.insert(name.to_owned(), spec.to_owned());
                }
            }
        }

        spec_map.retain(|name, _| installed.contains(name));
        Ok(spec_map)
    }

    // ── Writing ───────────────────────────────────────────────────────────

    /// Write a new transaction header (timestamp + command) to `writer`.
    ///
    /// `timestamp` must be in `YYYY-MM-DD HH:MM:SS` format.
    pub fn write_head(writer: &mut impl Write, timestamp: &str, cmd: &[&str]) -> io::Result<()> {
        writeln!(writer, "==> {timestamp} <==")?;
        writeln!(writer, "# cmd: {}", cmd.join(" "))?;
        Ok(())
    }

    /// Append a diff between `last_state` and `current_state` to the history
    /// file, creating it (and `conda-meta/`) if necessary.
    ///
    /// `timestamp` must be in `YYYY-MM-DD HH:MM:SS` format.
    pub fn write_changes(
        &self,
        timestamp: &str,
        last_state: &HashSet<String>,
        current_state: &HashSet<String>,
    ) -> Result<(), HistoryError> {
        let mut writer = self.open_for_append()?;
        writeln!(writer, "==> {timestamp} <==")?;

        let mut removed: Vec<&str> = last_state
            .iter()
            .filter(|p| !current_state.contains(*p))
            .map(String::as_str)
            .collect();
        removed.sort_unstable();

        let mut added: Vec<&str> = current_state
            .iter()
            .filter(|p| !last_state.contains(*p))
            .map(String::as_str)
            .collect();
        added.sort_unstable();

        for dist in removed {
            writeln!(writer, "-{dist}")?;
        }
        for dist in added {
            writeln!(writer, "+{dist}")?;
        }

        Ok(())
    }

    /// Append spec comment lines to the history file.
    ///
    /// Any of `remove_specs`, `update_specs`, or `neutered_specs` that are
    /// non-empty will each produce one comment line.
    pub fn write_specs(
        &self,
        remove_specs: &[&str],
        update_specs: &[&str],
        neutered_specs: &[&str],
    ) -> Result<(), HistoryError> {
        if remove_specs.is_empty() && update_specs.is_empty() && neutered_specs.is_empty() {
            return Ok(());
        }

        let mut writer = self.open_for_append()?;

        for (label, specs) in [
            ("remove", remove_specs),
            ("update", update_specs),
            ("neutered", neutered_specs),
        ] {
            if !specs.is_empty() {
                let formatted: Vec<String> = specs.iter().map(|s| format!("'{s}'")).collect();
                writeln!(writer, "# {label} specs: [{}]", formatted.join(", "))?;
            }
        }

        Ok(())
    }

    /// Open the history file for appending, creating it (and `conda-meta/`)
    /// if either does not exist.
    fn open_for_append(&self) -> Result<BufWriter<std::fs::File>, HistoryError> {
        if !self.meta_dir.exists() {
            std::fs::create_dir_all(&self.meta_dir)?;
        }
        let file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)?;
        Ok(BufWriter::new(file))
    }
}

// ── Free parsing functions ────────────────────────────────────────────────────

/// Parse a history file from a string slice.
///
/// Comment lines that appear before the first section header are ignored.
pub fn parse_str(content: &str) -> Result<Vec<HistoryEntry>, HistoryError> {
    let mut entries: Vec<HistoryEntry> = Vec::new();

    for line in content.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }

        if let Some(caps) = SECTION_HEADER_RE.captures(line) {
            let date_str = &caps[1];
            let date = NaiveDateTime::parse_from_str(date_str, "%Y-%m-%d %H:%M:%S")
                .map_err(|source| HistoryError::InvalidTimestamp {
                    timestamp: date_str.to_owned(),
                    source,
                })?;
            entries.push(HistoryEntry {
                date,
                packages: HashSet::new(),
                comments: Vec::new(),
            });
        } else if line.starts_with('#') {
            if let Some(entry) = entries.last_mut() {
                entry.comments.push(parse_comment_line(line));
            }
        } else if let Some(entry) = entries.last_mut() {
            entry.packages.insert(line.to_owned());
        }
    }

    Ok(entries)
}

/// Parse a single `#`-prefixed comment line into a [`HistoryCommentLine`].
pub fn parse_comment_line(line: &str) -> HistoryCommentLine {
    if let Some(caps) = CMD_RE.captures(line) {
        let mut argv: Vec<String> = caps[1].split_whitespace().map(str::to_owned).collect();
        // Normalise the executable name to just "conda".
        if let Some(first) = argv.first_mut() {
            if first.ends_with("conda") || first.ends_with("conda.exe") {
                *first = "conda".to_owned();
            }
        }
        return HistoryCommentLine::Cmd(argv);
    }

    if let Some(caps) = CONDA_VERSION_RE.captures(line) {
        return HistoryCommentLine::CondaVersion(caps[1].trim().to_owned());
    }

    if let Some(caps) = SPECS_RE.captures(line) {
        let action: HistoryAction = caps[1].trim().parse().expect("infallible");
        let specs = parse_specs_string(caps[2].trim())
            .into_iter()
            .filter(|s| !s.is_empty() && !s.ends_with('@'))
            .collect::<Vec<_>>();

        let specs_comment = match action {
            HistoryAction::Install | HistoryAction::Update => SpecsComment::Update(specs),
            HistoryAction::Remove => SpecsComment::Remove(specs),
            HistoryAction::Neutered => SpecsComment::Neutered(specs),
            HistoryAction::Other(name) => SpecsComment::Other {
                action: name,
                specs,
            },
        };
        return HistoryCommentLine::Specs(specs_comment);
    }

    // Strip the leading `#` and preserve the rest verbatim.
    let body = line.trim_start_matches('#').trim().to_owned();
    HistoryCommentLine::Other(body)
}

// ── Private parsing helpers ───────────────────────────────────────────────────

/// Parse a specs string in either the modern Python list syntax
/// (`['foo >=1.0', 'bar']`) or the legacy comma-separated syntax.
fn parse_specs_string(s: &str) -> Vec<String> {
    if s.starts_with('[') {
        parse_list_format_specs(s)
    } else {
        parse_legacy_specs(s)
    }
}

/// Parse specs from a Python list literal: `['foo>=1.0', 'bar']`.
fn parse_list_format_specs(s: &str) -> Vec<String> {
    let inner = s.trim_matches(|c| c == '[' || c == ']').trim();
    if inner.is_empty() {
        return Vec::new();
    }

    let specs: Vec<String> = QUOTED_ITEM_RE
        .captures_iter(inner)
        .map(|cap| cap[1].trim().to_owned())
        .filter(|s| !s.is_empty())
        .collect();

    // Fallback: bare unquoted string inside brackets.
    if specs.is_empty() {
        vec![inner.trim_matches(|c| c == '\'' || c == '"').to_owned()]
    } else {
        specs
    }
}

/// Parse legacy (pre-4.5) comma-separated spec strings.
///
/// A comma followed by a version-relation character (`<`, `>`, `=`, `!`)
/// belongs to the *previous* spec rather than starting a new one, e.g.
/// `numpy>=1.0,<2.0` is one spec, not two.
fn parse_legacy_specs(s: &str) -> Vec<String> {
    let mut specs: Vec<String> = Vec::new();
    for part in s.split(',') {
        let part = part.trim();
        if part.is_empty() {
            continue;
        }
        if VERSION_RELATION_RE.is_match(part) {
            if let Some(last) = specs.last_mut() {
                last.push(',');
                last.push_str(part);
                continue;
            }
        }
        specs.push(part.to_owned());
    }
    specs
}

// ── Private helper functions ──────────────────────────────────────────────────

/// Extract the package name from a spec string, stripping any channel prefix.
///
/// Returns a slice into the original string — callers that need an owned copy
/// can call `.to_owned()` on the result.
///
/// Handles specs like `foo`, `foo>=1.0`, `foo >=1.0`, `channel::foo>=1.0`.
fn package_name_from_spec(spec: &str) -> Option<&str> {
    // Strip optional channel prefix (`conda-forge::foo` → `foo`).
    let name_part = spec.find("::").map_or(spec, |pos| &spec[pos + 2..]);

    let end = name_part
        .find(['>', '<', '=', '!', ' ', '\t', '['])
        .unwrap_or(name_part.len());

    let name = &name_part[..end];
    if name.is_empty() { None } else { Some(name) }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;

    #[test]
    fn test_parse_history() {
        insta::glob!("../../../test-data/history", "*.history", |path| {
            let content = std::fs::read_to_string(path).unwrap();
            let entries = parse_str(&content).unwrap();
            insta::assert_yaml_snapshot!(entries
                .iter()
                .map(|e| {
                    let mut map = std::collections::BTreeMap::new();
                    map.insert("date", e.date.format("%Y-%m-%d %H:%M:%S").to_string());
                    map.insert("is_diff", e.is_diff().to_string());
                    map.insert("package_count", e.packages.len().to_string());
                    map.insert("comment_count", e.comments.len().to_string());
                    map
                })
                .collect::<Vec<_>>());
        });
    }

    #[test]
    fn test_parse_comment_line_cmd() {
        let parsed = parse_comment_line("# cmd: /usr/bin/conda install numpy pandas");
        assert!(
            matches!(parsed, HistoryCommentLine::Cmd(ref argv) if argv == &["conda", "install", "numpy", "pandas"])
        );
    }

    #[test]
    fn test_parse_comment_line_conda_version() {
        let parsed = parse_comment_line("# conda version: 25.7.0");
        assert!(matches!(parsed, HistoryCommentLine::CondaVersion(ref v) if v == "25.7.0"));
    }

    #[test]
    fn test_parse_comment_line_update_specs_list() {
        let parsed = parse_comment_line("# update specs: ['numpy>=1.0', 'pandas']");
        assert!(
            matches!(parsed, HistoryCommentLine::Specs(SpecsComment::Update(ref s)) if s == &["numpy>=1.0", "pandas"])
        );
    }

    #[test]
    fn test_parse_comment_line_remove_specs() {
        let parsed = parse_comment_line("# remove specs: ['conda-auth']");
        assert!(
            matches!(parsed, HistoryCommentLine::Specs(SpecsComment::Remove(ref s)) if s == &["conda-auth"])
        );
    }

    #[test]
    fn test_parse_comment_line_neutered_specs() {
        let parsed = parse_comment_line("# neutered specs: ['conda==25.9.1']");
        assert!(
            matches!(parsed, HistoryCommentLine::Specs(SpecsComment::Neutered(ref s)) if s == &["conda==25.9.1"])
        );
    }

    #[test]
    fn test_parse_legacy_specs() {
        let specs = parse_legacy_specs("numpy>=1.0,<2.0,pandas >=1.5");
        assert_eq!(specs, ["numpy>=1.0,<2.0", "pandas >=1.5"]);
    }

    #[test]
    fn test_is_diff() {
        let make_entry = |pkgs: &[&str]| HistoryEntry {
            date: NaiveDateTime::parse_from_str("2024-01-01 00:00:00", "%Y-%m-%d %H:%M:%S")
                .unwrap(),
            packages: pkgs.iter().map(|s| s.to_string()).collect(),
            comments: Vec::new(),
        };

        assert!(make_entry(&["+defaults::numpy-1.0-py39_0"]).is_diff());
        assert!(!make_entry(&["defaults::numpy-1.0-py39_0"]).is_diff());
    }

    #[test]
    fn test_package_name_from_spec() {
        assert_eq!(package_name_from_spec("numpy>=1.0"), Some("numpy"));
        assert_eq!(
            package_name_from_spec("conda-forge::numpy>=1.0"),
            Some("numpy")
        );
        assert_eq!(package_name_from_spec("conda==25.9.1"), Some("conda"));
        assert_eq!(package_name_from_spec(""), None);
    }

    #[test]
    fn test_construct_states_from_test_file() {
        let path =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../test-data/history/test.history");
        if !path.exists() {
            return;
        }
        let content = std::fs::read_to_string(&path).unwrap();
        let entries = parse_str(&content).unwrap();
        assert!(
            !entries.is_empty(),
            "expected at least one entry in test.history"
        );

        let mut current = HashSet::<String>::new();
        for entry in &entries {
            if entry.is_diff() {
                for pkg in &entry.packages {
                    if let Some(dist) = pkg.strip_prefix('+') {
                        current.insert(dist.to_owned());
                    } else if let Some(dist) = pkg.strip_prefix('-') {
                        current.remove(dist);
                    }
                }
            } else {
                current = entry.packages.clone();
            }
        }
        assert!(!current.is_empty(), "final state should not be empty");
    }
}
