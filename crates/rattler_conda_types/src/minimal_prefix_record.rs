//! Minimal prefix record reading for fast environment change detection.
//!
//! This module provides functionality to read only the minimal metadata needed
//! from conda-meta JSON files to determine if packages have changed, avoiding
//! the expensive parsing of file lists and other large fields of `PrefixRecord`.
//!
//! The most interesting part of this file is custom parser written
//! using `nom`. It uses `PrefixRecord` structure to parse as little
//! data as possible, also uses byte parsing on memory map, so we
//! don't read file entirely. See documentation of
//! [`MinimalPrefixRecord::from_path`] for more information.

use std::str::FromStr;
use std::{io, path::Path};

use crate::{NoArchType, PackageName, PackageRecord, PrefixRecord, VersionWithSource};
use hex;
use itertools::Itertools;
use memmap2::Mmap;
use nom::{
    bytes::complete::{tag, take_while},
    combinator::opt,
    multi::separated_list0,
    sequence::{delimited, preceded},
    IResult, Parser,
};
use rattler_digest::{Md5Hash, Sha256Hash};

/// A minimal version of `PrefixRecord` that only contains fields needed for transaction computation.
/// This is much faster to parse than the full `PrefixRecord`.
#[derive(Debug, Clone)]
#[allow(deprecated)]
pub struct MinimalPrefixRecord {
    /// The package name
    pub name: PackageName,
    /// The package version
    pub version: VersionWithSource,
    /// The build string
    pub build: String,

    /// SHA256 hash of the package
    pub sha256: Option<Sha256Hash>,
    /// MD5 hash of the package, only if there is no SHA256 hash.
    pub md5: Option<Md5Hash>,
    /// Size of the package in bytes, only if there is no MD5 hash.
    pub size: Option<u64>,

    /// If this package is independent of architecture this field specifies in
    /// what way. See [`NoArchType`] for more information.
    pub noarch: NoArchType,
    /// Optionally a path within the environment of the site-packages directory.
    /// This field is only present for python interpreter packages.
    /// This field was introduced with <https://github.com/conda/ceps/blob/main/cep-17.md>.
    pub python_site_packages_path: Option<String>,

    /// Deprecated: Old field for requested spec.
    /// Only used for migration to `requested_specs`.
    #[deprecated = "Use requested_specs instead"]
    pub requested_spec: Option<String>,

    /// The list of requested specs that were used to install this package.
    /// This is used to track which specs requested this package.
    pub requested_specs: Vec<String>,
}

impl MinimalPrefixRecord {
    // Ideal approach would be to create `SparsePrefixRecord` akin `SpareRepodata`.
    /// Parse a minimal prefix record from a JSON file using nom with memory mapping.
    /// This is optimized for performance and stops parsing at the "files" field.
    ///
    /// The corner stone of its logic is decision of whether or not we
    /// want to parse file completely.
    ///
    /// If we already met `requested_specs` field and now we're at
    /// `files`, then we stop as this means new `PrefixRecord` format with
    /// reordered fields and mandatory serialization of empty
    /// `requested_spec`.
    ///
    /// If we at `files`, but haven't met `requested_specs`, then we have
    /// legacy file format, so continue parsing either until we find this
    /// field or until the end of file.
    pub fn from_path(path: &Path) -> Result<Self, io::Error> {
        use std::fs::File;

        let filename_without_ext = path
            .file_stem()
            .and_then(|stem| stem.to_str())
            .ok_or_else(|| io::Error::other("Invalid filename"))?;
        let (build, version, name) = filename_without_ext
            .rsplitn(3, '-')
            .next_tuple()
            .ok_or_else(|| io::Error::other("Invalid conda-meta filename format"))?;

        let file = File::open(path)?;
        let initial_metadata = file.metadata()?;

        // SAFETY: We create a read-only memory map of a regular file.
        // We capture the file's metadata to detect if it changes during parsing.
        let mmap = unsafe { Mmap::map(&file)? };

        let parsed = parse_minimal_json(&mmap[..])
            .map_err(|e| io::Error::other(format!("Failed to parse JSON: {e}")));

        // Verify file hasn't changed during parsing
        let final_metadata = file.metadata()?;
        if initial_metadata.modified()? != final_metadata.modified()?
            || initial_metadata.len() != final_metadata.len()
        {
            return Err(io::Error::other("File was modified during parsing"));
        }

        let parsed = parsed?;

        let version = VersionWithSource::from_str(version)
            .map_err(|e| io::Error::other(format!("Failed to parse version: {e}")))?;

        let (final_sha256, final_md5, final_size) = if parsed.sha256.is_some() {
            (parsed.sha256, None, None)
        } else if parsed.md5.is_some() {
            (None, parsed.md5, None)
        } else {
            (None, None, parsed.size)
        };

        let final_python_site_packages_path = if name.trim() == "python" {
            parsed.python_site_packages_path.and_then(|quoted_bytes| {
                // Use serde_json directly on the quoted byte slice - no allocation!
                if let Ok(json_str) = std::str::from_utf8(quoted_bytes) {
                    serde_json::from_str::<String>(json_str).ok()
                } else {
                    None
                }
            })
        } else {
            None
        };

        // Properly unescape requested_spec if present
        let final_requested_spec = parsed.requested_spec.and_then(|quoted_bytes| {
            if let Ok(json_str) = std::str::from_utf8(quoted_bytes) {
                serde_json::from_str::<String>(json_str).ok()
            } else {
                None
            }
        });

        // Properly unescape requested_specs
        let final_requested_specs = parsed
            .requested_specs
            .into_iter()
            .filter_map(|quoted_bytes| {
                if let Ok(json_str) = std::str::from_utf8(quoted_bytes) {
                    serde_json::from_str::<String>(json_str).ok()
                } else {
                    None
                }
            })
            .collect();

        // Parse noarch field
        let final_noarch = match parsed.noarch {
            Some(quoted_bytes) => {
                if let Ok(json_str) = std::str::from_utf8(quoted_bytes) {
                    match serde_json::from_str::<String>(json_str) {
                        Ok(s) => match s.as_str() {
                            "python" => NoArchType::python(),
                            "generic" => NoArchType::generic(),
                            _ => NoArchType::none(),
                        },
                        Err(_) => NoArchType::none(),
                    }
                } else {
                    NoArchType::none()
                }
            }
            None => NoArchType::none(),
        };

        #[allow(deprecated)]
        Ok(Self {
            name: name
                .parse::<PackageName>()
                .map_err(|e| format!("Could not parse package name: {e:#?}"))
                .map_err(io::Error::other)?,
            version,
            build: build.into(),
            sha256: final_sha256,
            md5: final_md5,
            size: final_size,
            noarch: final_noarch,
            python_site_packages_path: final_python_site_packages_path,
            requested_specs: final_requested_specs,
            requested_spec: final_requested_spec,
        })
    }

    /// Convert to a partial `PackageRecord` for use in transaction computation.
    /// This creates a `PackageRecord` with only the essential fields filled in.
    pub fn to_package_record(&self) -> PackageRecord {
        PackageRecord {
            name: self.name.clone(),
            version: self.version.clone(),
            build: self.build.clone(),
            build_number: 0,
            subdir: "noarch".to_string(),
            md5: self.md5,
            sha256: self.sha256,
            size: self.size,
            noarch: NoArchType::none(),
            arch: None,
            platform: None,
            depends: Vec::new(),
            constrains: Vec::new(),
            features: None,
            legacy_bz2_size: None,
            license: None,
            license_family: None,
            purls: None,
            run_exports: None,
            timestamp: None,
            track_features: Vec::new(),
            python_site_packages_path: None,
            experimental_extra_depends: std::collections::BTreeMap::new(),
            legacy_bz2_md5: None,
        }
    }

    /// Create a `MinimalPrefixRecord` from a full `PrefixRecord` for comparison purposes
    pub fn from_prefix_record(prefix_record: &PrefixRecord) -> Self {
        let package_record = &prefix_record.repodata_record.package_record;

        let (final_sha256, final_md5, final_size) = if package_record.sha256.is_some() {
            (package_record.sha256, None, None)
        } else if package_record.md5.is_some() {
            (None, package_record.md5, None)
        } else {
            (None, None, package_record.size)
        };

        let final_python_site_packages_path = if package_record.name.as_source() == "python" {
            package_record.python_site_packages_path.clone()
        } else {
            None
        };

        #[allow(deprecated)]
        Self {
            name: package_record.name.clone(),
            version: package_record.version.clone(),
            build: package_record.build.clone(),
            sha256: final_sha256,
            md5: final_md5,
            size: final_size,
            noarch: package_record.noarch,
            python_site_packages_path: final_python_site_packages_path,
            requested_spec: prefix_record.requested_spec.clone(),
            requested_specs: prefix_record.requested_specs.clone(),
        }
    }
}

/// Collect minimal prefix records from a prefix directory.
/// This is much faster than collecting full `PrefixRecord`s when you only need
/// to check if packages have changed.
pub fn collect_minimal_prefix_records(
    prefix: &Path,
) -> Result<Vec<MinimalPrefixRecord>, io::Error> {
    let conda_meta_path = prefix.join("conda-meta");

    if !conda_meta_path.exists() {
        return Ok(Vec::new());
    }

    // Collect paths first
    let json_paths: Vec<_> = fs_err::read_dir(&conda_meta_path)?
        .filter_map(|entry| {
            entry.ok().and_then(|e| {
                if e.file_type().ok()?.is_file()
                    && e.file_name().to_string_lossy().ends_with(".json")
                {
                    Some(e.path())
                } else {
                    None
                }
            })
        })
        .collect();

    // Parse minimal records in parallel if rayon is available
    #[cfg(feature = "rayon")]
    {
        use rayon::prelude::*;
        json_paths
            .par_iter()
            .map(|path| MinimalPrefixRecord::from_path(path))
            .collect()
    }

    #[cfg(not(feature = "rayon"))]
    {
        json_paths
            .iter()
            .map(|path| MinimalPrefixRecord::from_path(path))
            .collect()
    }
}

/// Extension trait for `PrefixRecord` to support sparse collection
pub trait MinimalPrefixCollection {
    /// Collect only the minimal fields needed for transaction computation.
    /// Fall back to full parsing if more fields needed!
    fn collect_minimal_from_prefix(prefix: &Path) -> Result<Vec<MinimalPrefixRecord>, io::Error>;
}

impl MinimalPrefixCollection for PrefixRecord {
    fn collect_minimal_from_prefix(prefix: &Path) -> Result<Vec<MinimalPrefixRecord>, io::Error> {
        collect_minimal_prefix_records(prefix)
    }
}

/// Struct to hold the parsed fields from JSON
#[derive(Debug, Default)]
struct ParsedFields<'a> {
    sha256: Option<Sha256Hash>,
    md5: Option<Md5Hash>,
    size: Option<u64>,
    python_site_packages_path: Option<&'a [u8]>, // Store quoted JSON string
    requested_specs: Vec<&'a [u8]>,              // Store quoted JSON strings
    requested_spec: Option<&'a [u8]>,            // Store quoted JSON string
    noarch: Option<&'a [u8]>, // Store quoted JSON string or raw for null/bool/other
}

/// Parsing state to track what fields we've found
#[derive(Debug, Default)]
struct ParseState {
    found_requested_specs: bool,
    found_files: bool,
}

/// Parse whitespace characters
fn multispace0_bytes(input: &[u8]) -> IResult<&[u8], &[u8]> {
    take_while(|c| c == b' ' || c == b'\t' || c == b'\n' || c == b'\r')(input)
}

/// Parse minimal JSON fields using nom, stopping at "files" field
fn parse_minimal_json(
    input: &[u8],
) -> Result<ParsedFields<'_>, nom::Err<nom::error::Error<&[u8]>>> {
    let (_, parsed) = parse_json_object(input)?;
    Ok(parsed)
}

/// Parse JSON object looking for specific fields
fn parse_json_object(input: &[u8]) -> IResult<&[u8], ParsedFields<'_>> {
    let mut fields = ParsedFields::default();
    let mut state = ParseState::default();

    let (input, _) = preceded(multispace0_bytes, tag(&b"{"[..])).parse(input)?;
    let (input, _) = multispace0_bytes(input)?;

    let (remaining, _) = parse_fields(input, &mut fields, &mut state)?;

    Ok((remaining, fields))
}

/// Parse JSON fields with stateful logic for `requested_specs` and files
fn parse_fields<'a>(
    mut input: &'a [u8],
    fields: &mut ParsedFields<'a>,
    state: &mut ParseState,
) -> IResult<&'a [u8], ()> {
    loop {
        let (rest, _) = multispace0_bytes(input)?;
        input = rest;

        if input.starts_with(b"}") {
            return Ok((input, ()));
        }

        let (rest, field_name) = parse_json_string_content(input)?;
        input = rest;

        match field_name {
            b"files" => {
                state.found_files = true;
                // If we've already found requested_specs, we can stop here
                if state.found_requested_specs {
                    return Ok((input, ()));
                }
                // Otherwise, we need to continue parsing to find requested_specs
            }
            b"requested_specs" => {
                state.found_requested_specs = true;
            }
            _ => {}
        }

        let (rest, _) = preceded(multispace0_bytes, tag(&b":"[..])).parse(input)?;
        let (rest, _) = multispace0_bytes(rest)?;
        input = rest;

        let (rest, _) = parse_field_value(input, field_name, fields)?;
        input = rest;

        // If we just parsed requested_specs and we already found files, we can stop
        if field_name == b"requested_specs" && state.found_files {
            return Ok((rest, ()));
        }

        let (rest, _) = multispace0_bytes(input)?;
        let (rest, _) = opt(tag(&b","[..])).parse(rest)?;
        let (rest, _) = multispace0_bytes(rest)?;
        input = rest;
    }
}

/// Parse JSON string value, returning the content without quotes
fn parse_json_string_content(input: &[u8]) -> IResult<&[u8], &[u8]> {
    let (input, _) = tag(&b"\""[..])(input)?;

    let mut i = 0;
    while i < input.len() {
        match input[i] {
            b'"' => {
                // Found closing quote
                return Ok((&input[i + 1..], &input[..i]));
            }
            b'\\' => {
                if i + 1 < input.len() {
                    i += 2;
                } else {
                    return Err(nom::Err::Error(nom::error::Error::new(
                        input,
                        nom::error::ErrorKind::Escaped,
                    )));
                }
            }
            _ => {
                i += 1;
            }
        }
    }

    Err(nom::Err::Error(nom::error::Error::new(
        input,
        nom::error::ErrorKind::Tag,
    )))
}

/// Parse JSON string value, returning the full quoted string for `serde_json`.
fn parse_json_string_quoted(input: &[u8]) -> IResult<&[u8], &[u8]> {
    let start = input;
    let (input, _) = tag(&b"\""[..])(input)?;

    let mut i = 0;
    while i < input.len() {
        match input[i] {
            b'"' => {
                // Found closing quote, return the full quoted string
                let end_pos = (input.as_ptr() as usize + i + 1) - start.as_ptr() as usize;
                return Ok((&input[i + 1..], &start[..end_pos]));
            }
            b'\\' => {
                if i + 1 < input.len() {
                    i += 2;
                } else {
                    return Err(nom::Err::Error(nom::error::Error::new(
                        input,
                        nom::error::ErrorKind::Escaped,
                    )));
                }
            }
            _ => {
                i += 1;
            }
        }
    }

    Err(nom::Err::Error(nom::error::Error::new(
        input,
        nom::error::ErrorKind::Tag,
    )))
}

/// Parse field value based on field name
fn parse_field_value<'a>(
    input: &'a [u8],
    field_name: &[u8],
    fields: &mut ParsedFields<'a>,
) -> IResult<&'a [u8], ()> {
    match field_name {
        b"sha256" => {
            let (rest, value) = parse_json_string_content(input)?;
            if let Ok(hex_str) = std::str::from_utf8(value) {
                if let Ok(bytes) = hex::decode(hex_str) {
                    if bytes.len() == 32 {
                        fields.sha256 = Some(Sha256Hash::from(
                            <[u8; 32]>::try_from(bytes.as_slice()).unwrap(),
                        ));
                    }
                }
            }
            Ok((rest, ()))
        }
        b"md5" => {
            let (rest, value) = parse_json_string_content(input)?;
            if let Ok(hex_str) = std::str::from_utf8(value) {
                if let Ok(bytes) = hex::decode(hex_str) {
                    if bytes.len() == 16 {
                        fields.md5 = Some(Md5Hash::from(
                            <[u8; 16]>::try_from(bytes.as_slice()).unwrap(),
                        ));
                    }
                }
            }
            Ok((rest, ()))
        }
        b"size" => {
            let (rest, value) = take_until_comma_or_brace(input)?;
            if let Ok(size_str) = std::str::from_utf8(value) {
                fields.size = size_str.trim().parse().ok();
            }
            Ok((rest, ()))
        }
        b"python_site_packages_path" => {
            // Handle both string and null values for python_site_packages_path
            let (rest, _) = multispace0_bytes(input)?;
            if rest.starts_with(b"null") {
                fields.python_site_packages_path = None;
                Ok((&rest[4..], ()))
            } else {
                let (rest, quoted_value) = parse_json_string_quoted(input)?;
                fields.python_site_packages_path = Some(quoted_value);
                Ok((rest, ()))
            }
        }
        b"requested_spec" => {
            // Handle both string and null values for requested_spec
            let (rest, _) = multispace0_bytes(input)?;
            if rest.starts_with(b"null") {
                fields.requested_spec = None;
                Ok((&rest[4..], ()))
            } else {
                let (rest, quoted_value) = parse_json_string_quoted(input)?;
                fields.requested_spec = Some(quoted_value);
                Ok((rest, ()))
            }
        }
        b"requested_specs" => {
            let (rest, specs) = parse_json_array(input)?;
            fields.requested_specs = specs;
            Ok((rest, ()))
        }
        b"noarch" => {
            // Handle string values like "python", "generic" or null/false
            let (rest, _) = multispace0_bytes(input)?;
            if rest.starts_with(b"null") || rest.starts_with(b"false") {
                fields.noarch = None;
                let consumed = if rest.starts_with(b"null") { 4 } else { 5 };
                Ok((&rest[consumed..], ()))
            } else if rest.starts_with(b"\"") {
                let (rest, quoted_value) = parse_json_string_quoted(input)?;
                fields.noarch = Some(quoted_value);
                Ok((rest, ()))
            } else {
                // Skip unknown noarch values
                let (rest, _) = skip_json_value(input)?;
                Ok((rest, ()))
            }
        }
        _ => {
            // Skip unknown fields
            let (rest, _) = skip_json_value(input)?;
            Ok((rest, ()))
        }
    }
}

/// Parse JSON array of strings, returning quoted byte slices
fn parse_json_array(input: &[u8]) -> IResult<&[u8], Vec<&[u8]>> {
    delimited(
        preceded(multispace0_bytes, tag(&b"["[..])),
        separated_list0(
            preceded(multispace0_bytes, tag(&b","[..])),
            preceded(multispace0_bytes, parse_json_string_quoted),
        ),
        preceded(multispace0_bytes, tag(&b"]"[..])),
    )
    .parse(input)
}

/// Skip any JSON value (string, number, object, array, boolean, null)
fn skip_json_value(input: &[u8]) -> IResult<&[u8], ()> {
    let (rest, _) = multispace0_bytes(input)?;

    if rest.starts_with(b"\"") {
        let (rest, _) = parse_json_string_content(rest)?;
        Ok((rest, ()))
    } else if rest.starts_with(b"[") {
        skip_json_array(rest)
    } else if rest.starts_with(b"{") {
        skip_json_object(rest)
    } else if rest.starts_with(b"true") {
        Ok((&rest[4..], ()))
    } else if rest.starts_with(b"false") {
        Ok((&rest[5..], ()))
    } else if rest.starts_with(b"null") {
        Ok((&rest[4..], ()))
    } else {
        // Number
        take_until_comma_or_brace(rest).map(|(rest, _)| (rest, ()))
    }
}

/// Skip JSON array
fn skip_json_array(input: &[u8]) -> IResult<&[u8], ()> {
    let mut input = input;
    let (rest, _) = tag(&b"["[..]).parse(input)?;
    input = rest;

    let mut depth = 1;
    while depth > 0 && !input.is_empty() {
        if input.starts_with(b"[") {
            depth += 1;
            input = &input[1..];
        } else if input.starts_with(b"]") {
            depth -= 1;
            input = &input[1..];
        } else {
            input = &input[1..];
        }
    }

    Ok((input, ()))
}

/// Skip JSON object
fn skip_json_object(input: &[u8]) -> IResult<&[u8], ()> {
    let mut input = input;
    let (rest, _) = tag(&b"{"[..]).parse(input)?;
    input = rest;

    let mut depth = 1;
    while depth > 0 && !input.is_empty() {
        if input.starts_with(b"{") {
            depth += 1;
            input = &input[1..];
        } else if input.starts_with(b"}") {
            depth -= 1;
            input = &input[1..];
        } else {
            input = &input[1..];
        }
    }

    Ok((input, ()))
}

/// Take bytes until comma, closing brace, or closing bracket
fn take_until_comma_or_brace(input: &[u8]) -> IResult<&[u8], &[u8]> {
    let mut end = 0;

    for &byte in input {
        if byte == b',' || byte == b'}' || byte == b']' {
            break;
        }
        end += 1;
    }

    Ok((&input[end..], &input[..end]))
}

#[allow(deprecated)]
#[cfg(test)]
mod tests {
    use super::*;

    use std::{fs, path::PathBuf};

    use rstest::rstest;

    #[test]
    fn test_stateful_parsing_requested_specs_before_files() {
        // Test case where requested_specs appears before files
        let temp_dir = tempfile::tempdir().unwrap();
        let file_path = temp_dir.path().join("stateful-before-1.0.0-build1.json");

        let json_content = r#"{
  "name": "stateful-before",
  "version": "1.0.0",
  "build": "build1",
  "sha256": "abcdef1234567890abcdef1234567890abcdef1234567890abcdef1234567890",
  "requested_specs": ["stateful-before >=1.0"],
  "files": [
    "file1.txt",
    "file2.txt"
  ],
  "this_field_after_files": "should_be_ignored"
}"#;

        fs::write(&file_path, json_content).unwrap();

        let result = MinimalPrefixRecord::from_path(&file_path).unwrap();

        assert_eq!(result.name.as_source(), "stateful-before");
        assert_eq!(result.requested_specs, vec!["stateful-before >=1.0"]);
    }

    #[test]
    fn test_stateful_parsing_requested_specs_after_files() {
        // Test case where requested_specs appears after files
        let temp_dir = tempfile::tempdir().unwrap();
        let file_path = temp_dir.path().join("stateful-after-1.0.0-build1.json");

        let json_content = r#"{
  "name": "stateful-after",
  "version": "1.0.0",
  "build": "build1",
  "sha256": "abcdef1234567890abcdef1234567890abcdef1234567890abcdef1234567890",
  "files": [
    "file1.txt",
    "file2.txt"
  ],
  "requested_specs": ["stateful-after >=1.0"],
  "this_field_after_requested_specs": "should_be_ignored"
}"#;

        fs::write(&file_path, json_content).unwrap();

        let result = MinimalPrefixRecord::from_path(&file_path).unwrap();

        assert_eq!(result.name.as_source(), "stateful-after");
        assert_eq!(result.requested_specs, vec!["stateful-after >=1.0"]);
    }

    #[test]
    fn test_stateful_parsing_no_requested_specs() {
        // Test case where requested_specs is missing entirely
        let temp_dir = tempfile::tempdir().unwrap();
        let file_path = temp_dir.path().join("stateful-missing-1.0.0-build1.json");

        let json_content = r#"{
  "name": "stateful-missing",
  "version": "1.0.0",
  "build": "build1",
  "sha256": "abcdef1234567890abcdef1234567890abcdef1234567890abcdef1234567890",
  "files": [
    "file1.txt",
    "file2.txt"
  ],
  "other_field": "value"
}"#;

        fs::write(&file_path, json_content).unwrap();

        let result = MinimalPrefixRecord::from_path(&file_path).unwrap();

        assert_eq!(result.name.as_source(), "stateful-missing");
        assert!(result.requested_specs.is_empty());
    }

    #[test]
    fn test_json_string_escaping() {
        // Test case with escaped characters in requested_specs
        let temp_dir = tempfile::tempdir().unwrap();
        let file_path = temp_dir.path().join("escape-test-1.0.0-build1.json");

        let json_content = r#"{
  "name": "escape-test",
  "version": "1.0.0",
  "build": "build1",
  "sha256": "abcdef1234567890abcdef1234567890abcdef1234567890abcdef1234567890",
  "requested_specs": ["escape-test >= \"1.0\"", "python-site \"lib/site\""],
  "python_site_packages_path": "lib/python3.9/site-packages",
  "files": ["file1.txt"]
}"#;

        fs::write(&file_path, json_content).unwrap();

        let result = MinimalPrefixRecord::from_path(&file_path).unwrap();

        assert_eq!(result.name.as_source(), "escape-test");

        // Verify that JSON escapes are properly handled during conversion
        assert_eq!(
            result.requested_specs,
            vec![r#"escape-test >= "1.0""#, r#"python-site "lib/site""#]
        );

        // python_site_packages_path should be None since package name is not "python"
        assert_eq!(result.python_site_packages_path, None);
    }

    #[rstest]
    fn test_minimal_prefix_record_vs_prefix_record_parsing(
        #[files("../../test-data/conda-meta/*.json")] test_file: PathBuf,
    ) {
        let file_name = test_file.file_name().unwrap().to_string_lossy();
        // Parse with full PrefixRecord
        let full_record = crate::PrefixRecord::from_path(&test_file).unwrap();

        // Parse with MinimalPrefixRecord
        let minimal_record = MinimalPrefixRecord::from_path(&test_file).unwrap();

        // Convert full record to minimal for comparison
        let minimal_from_full = MinimalPrefixRecord::from_prefix_record(&full_record);

        // Compare core fields
        assert_eq!(
            minimal_record.name, minimal_from_full.name,
            "name mismatch in {file_name}"
        );
        assert_eq!(
            minimal_record.version, minimal_from_full.version,
            "version mismatch in {file_name}"
        );
        assert_eq!(
            minimal_record.build, minimal_from_full.build,
            "build mismatch in {file_name}"
        );
        assert_eq!(
            minimal_record.sha256, minimal_from_full.sha256,
            "sha256 mismatch in {file_name}"
        );
        assert_eq!(
            minimal_record.md5, minimal_from_full.md5,
            "md5 mismatch in {file_name}"
        );
        assert_eq!(
            minimal_record.size, minimal_from_full.size,
            "size mismatch in {file_name}"
        );
        assert_eq!(
            minimal_record.python_site_packages_path, minimal_from_full.python_site_packages_path,
            "python_site_packages_path mismatch in {file_name}"
        );
        assert_eq!(
            minimal_record.requested_spec, minimal_from_full.requested_spec,
            "requested_spec mismatch in {file_name}"
        );
        assert_eq!(
            minimal_record.requested_specs, minimal_from_full.requested_specs,
            "requested_specs mismatch in {file_name}"
        );
    }
}
