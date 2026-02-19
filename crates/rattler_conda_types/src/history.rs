//! Readers and writers for `conda-meta/history` files.
//!
//! Conda environments keep a `conda-meta/history` file that records every
//! change (revision) made to the environment. Each revision contains a
//! timestamp header, optional comment lines (the command that was run, the
//! conda version, and the specs that were requested), and a list of packages.
//! Packages are either listed as an absolute set (for the initial creation) or
//! as diffs prefixed with `+` (added) or `-` (removed).
//!
//! This module provides:
//! - [`History`] — the main entry-point for reading and writing history files.
//! - [`ParsedHistory`] — the parsed contents of a history file, with query methods.
//! - [`HistoryRevision`] — a single parsed revision.
//! - [`UserRequest`] — structured data extracted from revision comments.

use std::{
    collections::BTreeSet,
    io::{Read, Write},
    path::{Path, PathBuf},
};

/// A single revision in a conda history file.
///
/// Each revision starts with a header line `==> <datetime> <==`, followed by
/// optional comment lines (prefixed with `#`) and package distribution strings.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HistoryRevision {
    /// The datetime string from the revision header.
    pub timestamp: String,

    /// The set of package distribution strings for this revision.
    ///
    /// In the initial revision these are bare distribution strings (e.g.
    /// `python-3.12.0-h1234567_0`). In subsequent revisions they may be
    /// prefixed with `+` (added) or `-` (removed).
    pub packages: BTreeSet<String>,

    /// Raw comment lines (including the leading `#`).
    pub comments: Vec<String>,
}

/// A structured representation of a user request extracted from comment lines
/// in a [`HistoryRevision`].
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct UserRequest {
    /// The datetime string from the revision header.
    pub date: String,

    /// The command that was run (from `# cmd: ...`).
    pub cmd: Option<String>,

    /// The conda version that was used (from `# conda version: ...`).
    pub conda_version: Option<String>,

    /// The action that was performed, e.g. `install`, `remove`, `update`,
    /// `create` (from `# <action> specs: ...`).
    pub action: Option<String>,

    /// Specs for install/update/create actions.
    pub update_specs: Vec<String>,

    /// Specs for remove/uninstall actions.
    pub remove_specs: Vec<String>,

    /// Specs that have been neutered (constrained).
    pub neutered_specs: Vec<String>,
}

/// Errors that can occur when working with history files.
#[derive(Debug, thiserror::Error)]
pub enum HistoryError {
    /// An I/O error occurred.
    #[error(transparent)]
    IoError(#[from] std::io::Error),

    /// A parse error occurred.
    #[error("failed to parse history: {0}")]
    ParseError(String),
}

/// A revision to be written to a history file.
///
/// This groups the data needed to append a single revision entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Revision {
    /// Timestamp string, typically in `YYYY-MM-DD HH:MM:SS` format.
    pub timestamp: String,

    /// Distribution strings that were removed in this revision.
    pub removed: BTreeSet<String>,

    /// Distribution strings that were added in this revision.
    pub added: BTreeSet<String>,
}

/// The parsed contents of a `conda-meta/history` file.
///
/// This type is returned by [`History::parse`] and provides query methods
/// over the parsed revisions.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedHistory {
    /// The list of revisions parsed from the file.
    pub revisions: Vec<HistoryRevision>,
}

impl ParsedHistory {
    /// Returns the number of revisions.
    pub fn len(&self) -> usize {
        self.revisions.len()
    }

    /// Returns `true` if there are no revisions.
    pub fn is_empty(&self) -> bool {
        self.revisions.is_empty()
    }

    /// Returns a reference to the latest (most recent) revision, if any.
    pub fn latest(&self) -> Option<&HistoryRevision> {
        self.revisions.last()
    }

    /// Extracts structured [`UserRequest`]s from the parsed revisions.
    ///
    /// A user request is produced for every revision that contains a
    /// `# cmd: ...` comment.
    pub fn user_requests(&self) -> Vec<UserRequest> {
        Self::user_requests_from_revisions(&self.revisions)
    }

    /// Extracts [`UserRequest`]s from an already-parsed list of revisions.
    pub fn user_requests_from_revisions(revisions: &[HistoryRevision]) -> Vec<UserRequest> {
        let cmd_re = lazy_regex::regex!(r"^#\s*cmd:\s*(.+)$");
        let conda_v_re = lazy_regex::regex!(r"^#\s*conda version:\s*(.+)$");
        let spec_re = lazy_regex::regex!(r"^#\s*(\w+)\s*specs:\s*(.+)?$");

        let mut requests = Vec::new();

        for rev in revisions {
            let mut req = UserRequest {
                date: rev.timestamp.clone(),
                ..Default::default()
            };

            for comment in &rev.comments {
                if let Some(caps) = cmd_re.captures(comment) {
                    req.cmd = Some(caps[1].to_string());
                }
                if let Some(caps) = conda_v_re.captures(comment) {
                    req.conda_version = Some(caps[1].to_string());
                }
                if let Some(caps) = spec_re.captures(comment) {
                    let action = caps[1].to_string();
                    let specs_str = caps.get(2).map_or("", |m| m.as_str());
                    let specs = parse_specs_string(specs_str);

                    match action.as_str() {
                        "install" | "create" | "update" => {
                            req.action = Some(action);
                            req.update_specs = specs;
                        }
                        "remove" | "uninstall" => {
                            req.action = Some(action);
                            req.remove_specs = specs;
                        }
                        "neutered" => {
                            req.action = Some(action);
                            req.neutered_specs = specs;
                        }
                        _ => {
                            req.action = Some(action);
                        }
                    }
                }
            }

            if req.cmd.is_some() {
                requests.push(req);
            }
        }

        requests
    }
}

/// Provides read and write access to a `conda-meta/history` file for a conda
/// environment prefix.
///
/// # Reading
///
/// Use [`History::parse`] to parse the file into a [`ParsedHistory`] and then
/// use its query methods (e.g. [`ParsedHistory::user_requests`]) to extract
/// structured data.
///
/// # Writing
///
/// Use [`History::write_revision`] to append a new revision. Use
/// [`History::clear`] to empty the file (useful for tools like pixi that do not
/// maintain a running history).
#[derive(Debug, Clone)]
pub struct History {
    /// Path to the `conda-meta/history` file.
    path: PathBuf,
}

impl History {
    /// Creates a new `History` pointing at `<prefix>/conda-meta/history`.
    pub fn new(prefix: impl AsRef<Path>) -> Self {
        Self {
            path: prefix.as_ref().join("conda-meta").join("history"),
        }
    }

    /// Creates a `History` from an explicit path to a history file.
    pub fn from_path(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    /// Returns the path to the history file.
    pub fn path(&self) -> &Path {
        &self.path
    }

    // -----------------------------------------------------------------------
    // Reader
    // -----------------------------------------------------------------------

    /// Parses the history file into a [`ParsedHistory`].
    ///
    /// Returns an empty [`ParsedHistory`] if the file does not exist or is
    /// empty. Comments appearing before the first revision header are
    /// silently ignored, which matches conda's behaviour.
    pub fn parse(&self) -> Result<ParsedHistory, HistoryError> {
        if !self.path.exists() {
            return Ok(ParsedHistory {
                revisions: Vec::new(),
            });
        }

        let contents = fs_err::read_to_string(&self.path)?;
        Self::parse_str(&contents)
    }

    /// Parses a history file from a reader.
    pub fn from_reader(mut reader: impl Read) -> Result<ParsedHistory, HistoryError> {
        let mut contents = String::new();
        reader.read_to_string(&mut contents)?;
        Self::parse_str(&contents)
    }

    /// Parses a history string into a [`ParsedHistory`].
    pub fn parse_str(s: &str) -> Result<ParsedHistory, HistoryError> {
        let sep_re = lazy_regex::regex!(r"^==>\s*(.+?)\s*<==$");

        let mut revisions = Vec::new();

        for line in s.lines() {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }

            if let Some(caps) = sep_re.captures(line) {
                let timestamp = caps[1].to_string();
                revisions.push(HistoryRevision {
                    timestamp,
                    packages: BTreeSet::new(),
                    comments: Vec::new(),
                });
            } else if line.starts_with('#') {
                // Attach comment to the current revision (ignore if before first header).
                if let Some(rev) = revisions.last_mut() {
                    rev.comments.push(line.to_string());
                }
            } else if let Some(rev) = revisions.last_mut() {
                rev.packages.insert(line.to_string());
            }
            // Lines before the first header are silently ignored.
        }

        Ok(ParsedHistory { revisions })
    }

    /// Extracts structured [`UserRequest`]s from the parsed history.
    ///
    /// This is a convenience method that parses the file and then calls
    /// [`ParsedHistory::user_requests`].
    pub fn get_user_requests(&self) -> Result<Vec<UserRequest>, HistoryError> {
        let parsed = self.parse()?;
        Ok(parsed.user_requests())
    }

    // -----------------------------------------------------------------------
    // Writer
    // -----------------------------------------------------------------------

    /// Creates a [`HistoryWriter`] that appends to the history file.
    ///
    /// The file is opened once and kept open for the lifetime of the writer,
    /// avoiding repeated open/close cycles when writing multiple entries.
    pub fn writer(&self) -> Result<HistoryWriter, HistoryError> {
        if let Some(parent) = self.path.parent() {
            fs_err::create_dir_all(parent)?;
        }

        let file = fs_err::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)?;

        Ok(HistoryWriter { file })
    }

    /// Convenience method: appends a single revision to the history file.
    ///
    /// Opens the file, writes, and closes it. For multiple writes consider
    /// using [`History::writer`] instead.
    pub fn write_revision(&self, revision: &Revision) -> Result<(), HistoryError> {
        self.writer()?.write_revision(revision)
    }

    /// Convenience method: writes a full revision with metadata.
    ///
    /// Opens the file, writes, and closes it. For multiple writes consider
    /// using [`History::writer`] instead.
    pub fn write_full_revision(
        &self,
        revision: &Revision,
        cmd: Option<&str>,
        conda_version: Option<&str>,
        action_specs: Option<(&str, &[String])>,
    ) -> Result<(), HistoryError> {
        self.writer()?
            .write_full_revision(revision, cmd, conda_version, action_specs)
    }

    /// Clears (truncates) the history file.
    ///
    /// This is useful for tools that do not maintain a running history but
    /// still need the file to exist (e.g. pixi).
    pub fn clear(&self) -> Result<(), HistoryError> {
        if let Some(parent) = self.path.parent() {
            fs_err::create_dir_all(parent)?;
        }

        fs_err::write(&self.path, "")?;
        Ok(())
    }

    /// Overwrites the history file with the given revisions.
    ///
    /// This completely replaces the file contents. It is the caller's
    /// responsibility to ensure the revisions are well-formed.
    pub fn overwrite(&self, revisions: &[HistoryRevision]) -> Result<(), HistoryError> {
        if let Some(parent) = self.path.parent() {
            fs_err::create_dir_all(parent)?;
        }

        let mut file = fs_err::File::create(&self.path)?;

        for rev in revisions {
            writeln!(file, "==> {} <==", rev.timestamp)?;
            for comment in &rev.comments {
                writeln!(file, "{comment}")?;
            }
            for pkg in &rev.packages {
                writeln!(file, "{pkg}")?;
            }
        }

        Ok(())
    }
}

/// A writer that keeps the history file open for efficient batch writes.
///
/// Obtain an instance via [`History::writer`].
pub struct HistoryWriter {
    file: fs_err::File,
}

impl HistoryWriter {
    /// Appends a revision entry to the history file.
    ///
    /// Packages in [`Revision::removed`] and [`Revision::added`] are written
    /// with the appropriate `+`/`-` prefix.
    pub fn write_revision(&mut self, revision: &Revision) -> Result<(), HistoryError> {
        writeln!(self.file, "==> {} <==", revision.timestamp)?;

        for pkg in &revision.removed {
            writeln!(self.file, "-{pkg}")?;
        }
        for pkg in &revision.added {
            writeln!(self.file, "+{pkg}")?;
        }

        Ok(())
    }

    /// Appends a comment line to the history file.
    ///
    /// The caller is responsible for formatting the comment correctly (the
    /// leading `#` should be included).
    pub fn write_comment(&mut self, comment: &str) -> Result<(), HistoryError> {
        writeln!(self.file, "{comment}")?;
        Ok(())
    }

    /// Writes a complete revision with optional metadata comments.
    pub fn write_full_revision(
        &mut self,
        revision: &Revision,
        cmd: Option<&str>,
        conda_version: Option<&str>,
        action_specs: Option<(&str, &[String])>,
    ) -> Result<(), HistoryError> {
        // Revision header.
        writeln!(self.file, "==> {} <==", revision.timestamp)?;

        // Comment metadata.
        if let Some(cmd) = cmd {
            writeln!(self.file, "# cmd: {cmd}")?;
        }
        if let Some(version) = conda_version {
            writeln!(self.file, "# conda version: {version}")?;
        }
        if let Some((action, specs)) = action_specs {
            writeln!(self.file, "# {action} specs: {specs:?}")?;
        }

        // Package diffs.
        for pkg in &revision.removed {
            writeln!(self.file, "-{pkg}")?;
        }
        for pkg in &revision.added {
            writeln!(self.file, "+{pkg}")?;
        }

        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Parses a specs string from a comment line.
///
/// The string may be in Python list format (e.g. `['numpy', 'pandas']`) or in
/// the older comma-separated format (e.g. `numpy,pandas>=1.5`).
fn parse_specs_string(s: &str) -> Vec<String> {
    let s = s.trim();
    if s.is_empty() {
        return Vec::new();
    }

    // Python list format: ['spec1', 'spec2']
    if s.starts_with('[') {
        return s
            .trim_start_matches('[')
            .trim_end_matches(']')
            .split(',')
            .map(|spec| spec.trim().trim_matches('\'').trim_matches('"').to_string())
            .filter(|spec| !spec.is_empty() && !spec.ends_with('@'))
            .collect();
    }

    // Older comma-separated format.
    // A version qualifier (>=, <=, etc.) after a comma belongs to the previous
    // spec, not a new one.
    let version_start_re = lazy_regex::regex!(r"^[><=!]");

    let mut specs = Vec::new();
    for part in s.split(',') {
        let part = part.trim();
        if part.is_empty() {
            continue;
        }
        if version_start_re.is_match(part) && !specs.is_empty() {
            // This is a continuation of the previous spec's version constraint.
            let last = specs.last_mut().unwrap();
            *last = format!("{last},{part}");
        } else {
            specs.push(part.to_string());
        }
    }

    specs
        .into_iter()
        .filter(|spec| !spec.is_empty() && !spec.ends_with('@'))
        .collect()
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use super::*;

    /// A sample history file matching conda's format.
    const SAMPLE_HISTORY: &str = "\
==> 2024-01-15 10:30:00 <==
# cmd: conda create -n test python=3.12
# conda version: 24.1.0
# install specs: ['python=3.12']
python-3.12.0-h1234567_0
openssl-3.2.0-h8765432_0
pip-24.0-pyhd8ed1ab_0

==> 2024-01-16 14:20:00 <==
# cmd: conda install numpy pandas
# conda version: 24.1.0
# update specs: ['numpy', 'pandas']
+numpy-1.26.3-py312h1234567_0
+pandas-2.1.5-py312h7654321_0
+python-dateutil-2.8.2-pyhd8ed1ab_0

==> 2024-01-17 09:00:00 <==
# cmd: conda remove pip
# conda version: 24.1.0
# remove specs: ['pip']
-pip-24.0-pyhd8ed1ab_0

==> 2024-01-18 10:00:00 <==
# cmd: conda install scipy
# conda version: 24.1.0
# neutered specs: ['numpy']
+scipy-1.11.4-py312h1234567_0
";

    #[test]
    fn parse_empty() {
        let parsed = History::parse_str("").unwrap();
        assert!(parsed.is_empty());
    }

    #[test]
    fn parse_revisions() {
        let parsed = History::parse_str(SAMPLE_HISTORY).unwrap();
        assert_eq!(parsed.len(), 4);

        let revisions = &parsed.revisions;

        // First revision: initial install (absolute packages).
        assert_eq!(revisions[0].timestamp, "2024-01-15 10:30:00");
        assert_eq!(revisions[0].packages.len(), 3);
        assert!(revisions[0].packages.contains("python-3.12.0-h1234567_0"));
        assert_eq!(revisions[0].comments.len(), 3);

        // Second revision: update (diff packages).
        assert_eq!(revisions[1].timestamp, "2024-01-16 14:20:00");
        assert_eq!(revisions[1].packages.len(), 3);
        assert!(revisions[1]
            .packages
            .contains("+numpy-1.26.3-py312h1234567_0"));

        // Third revision: remove (diff packages).
        assert_eq!(revisions[2].timestamp, "2024-01-17 09:00:00");
        assert_eq!(revisions[2].packages.len(), 1);
        assert!(revisions[2].packages.contains("-pip-24.0-pyhd8ed1ab_0"));

        // Fourth revision: neutered specs.
        assert_eq!(revisions[3].timestamp, "2024-01-18 10:00:00");
        assert_eq!(revisions[3].packages.len(), 1);
        assert!(revisions[3]
            .packages
            .contains("+scipy-1.11.4-py312h1234567_0"));
    }

    #[test]
    fn parse_user_requests() {
        let parsed = History::parse_str(SAMPLE_HISTORY).unwrap();
        let requests = parsed.user_requests();
        assert_eq!(requests.len(), 4);

        // First request: create.
        assert_eq!(requests[0].date, "2024-01-15 10:30:00");
        assert_eq!(
            requests[0].cmd.as_deref(),
            Some("conda create -n test python=3.12")
        );
        assert_eq!(requests[0].conda_version.as_deref(), Some("24.1.0"));
        assert_eq!(requests[0].action.as_deref(), Some("install"));
        assert_eq!(requests[0].update_specs, vec!["python=3.12"]);

        // Second request: install.
        assert_eq!(requests[1].action.as_deref(), Some("update"));
        assert_eq!(requests[1].update_specs, vec!["numpy", "pandas"]);

        // Third request: remove.
        assert_eq!(requests[2].action.as_deref(), Some("remove"));
        assert_eq!(requests[2].remove_specs, vec!["pip"]);

        // Fourth request: neutered.
        assert_eq!(requests[3].action.as_deref(), Some("neutered"));
        assert_eq!(requests[3].neutered_specs, vec!["numpy"]);
    }

    #[test]
    fn parse_ignores_lines_before_first_header() {
        let input = "some random line\n# a comment\n==> 2024-01-01 00:00:00 <==\npkg-1.0-0\n";
        let parsed = History::parse_str(input).unwrap();
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed.revisions[0].packages.len(), 1);
        assert!(parsed.revisions[0].comments.is_empty());
    }

    #[test]
    fn parse_specs_python_list_format() {
        let specs = parse_specs_string("['numpy', 'pandas>=1.5']");
        assert_eq!(specs, vec!["numpy", "pandas>=1.5"]);
    }

    #[test]
    fn parse_specs_old_comma_format() {
        let specs = parse_specs_string("param >=1.5.1,<2.0,python>=3.5");
        assert_eq!(specs, vec!["param >=1.5.1,<2.0", "python>=3.5"]);
    }

    #[test]
    fn parse_specs_empty() {
        let specs = parse_specs_string("");
        assert!(specs.is_empty());
    }

    #[test]
    fn parse_specs_filters_at_suffix() {
        let specs = parse_specs_string("['numpy', 'test@']");
        assert_eq!(specs, vec!["numpy"]);
    }

    #[test]
    fn write_and_read_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let history = History::new(dir.path());

        let rev = Revision {
            timestamp: "2024-01-15 10:30:00".to_string(),
            removed: BTreeSet::new(),
            added: BTreeSet::from([
                "python-3.12.0-h1234567_0".to_string(),
                "pip-24.0-pyhd8ed1ab_0".to_string(),
            ]),
        };

        history.write_revision(&rev).unwrap();

        let parsed = history.parse().unwrap();
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed.revisions[0].packages.len(), 2);
        assert!(parsed.revisions[0]
            .packages
            .contains("+pip-24.0-pyhd8ed1ab_0"));
        assert!(parsed.revisions[0]
            .packages
            .contains("+python-3.12.0-h1234567_0"));
    }

    #[test]
    fn write_full_revision_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let history = History::new(dir.path());

        let rev = Revision {
            timestamp: "2024-02-01 08:00:00".to_string(),
            removed: BTreeSet::new(),
            added: BTreeSet::from(["numpy-1.26.3-py312h1234567_0".to_string()]),
        };

        let specs = vec!["numpy".to_string()];
        history
            .write_full_revision(
                &rev,
                Some("conda install numpy"),
                Some("24.1.0"),
                Some(("install", &specs)),
            )
            .unwrap();

        let parsed = history.parse().unwrap();
        assert_eq!(parsed.len(), 1);
        assert!(!parsed.revisions[0].comments.is_empty());

        let requests = parsed.user_requests();
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].cmd.as_deref(), Some("conda install numpy"));
        assert_eq!(requests[0].conda_version.as_deref(), Some("24.1.0"));
    }

    #[test]
    fn clear_truncates_file() {
        let dir = tempfile::tempdir().unwrap();
        let history = History::new(dir.path());

        // Write something first.
        let rev = Revision {
            timestamp: "2024-01-01 00:00:00".to_string(),
            removed: BTreeSet::new(),
            added: BTreeSet::from(["pkg-1.0-0".to_string()]),
        };
        history.write_revision(&rev).unwrap();
        assert!(!history.parse().unwrap().is_empty());

        // Clear.
        history.clear().unwrap();
        assert!(history.parse().unwrap().is_empty());
    }

    #[test]
    fn parsed_history_latest() {
        let parsed = History::parse_str(SAMPLE_HISTORY).unwrap();
        let latest = parsed.latest().unwrap();
        assert_eq!(latest.timestamp, "2024-01-17 09:00:00");
    }

    #[test]
    fn overwrite_replaces_content() {
        let dir = tempfile::tempdir().unwrap();
        let history = History::new(dir.path());

        // Write initial content.
        let rev = Revision {
            timestamp: "2024-01-01 00:00:00".to_string(),
            removed: BTreeSet::new(),
            added: BTreeSet::from(["old-pkg-1.0-0".to_string()]),
        };
        history.write_revision(&rev).unwrap();

        // Overwrite with new revisions.
        let new_revisions = vec![HistoryRevision {
            timestamp: "2024-06-01 12:00:00".to_string(),
            packages: BTreeSet::from(["new-pkg-2.0-0".to_string()]),
            comments: vec!["# cmd: conda install new-pkg".to_string()],
        }];
        history.overwrite(&new_revisions).unwrap();

        let parsed = history.parse().unwrap();
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed.revisions[0].timestamp, "2024-06-01 12:00:00");
        assert!(parsed.revisions[0].packages.contains("new-pkg-2.0-0"));
    }

    #[test]
    fn parse_nonexistent_file() {
        let history = History::from_path("/nonexistent/path/history");
        let parsed = history.parse().unwrap();
        assert!(parsed.is_empty());
    }

    #[test]
    fn write_multiple_revisions() {
        let dir = tempfile::tempdir().unwrap();
        let history = History::new(dir.path());

        // First revision.
        let rev1 = Revision {
            timestamp: "2024-01-01 00:00:00".to_string(),
            removed: BTreeSet::new(),
            added: BTreeSet::from(["python-3.12.0-h1234567_0".to_string()]),
        };
        history.write_revision(&rev1).unwrap();

        // Second revision.
        let rev2 = Revision {
            timestamp: "2024-01-02 12:00:00".to_string(),
            removed: BTreeSet::from(["python-3.12.0-h1234567_0".to_string()]),
            added: BTreeSet::from(["python-3.13.0-h1234567_0".to_string()]),
        };
        history.write_revision(&rev2).unwrap();

        let parsed = history.parse().unwrap();
        assert_eq!(parsed.len(), 2);
    }
}
