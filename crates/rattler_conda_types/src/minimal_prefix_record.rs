//! Minimal prefix record reading for fast environment change detection.
//!
//! This module provides functionality to read only the minimal metadata needed
//! from conda-meta JSON files to determine if packages have changed, avoiding
//! the expensive parsing of file lists and other large data structures.

use std::str::FromStr;
use std::{io, path::Path};

use crate::{NoArchType, PackageName, PackageRecord, PrefixRecord, Version, VersionWithSource};
use hex;
use itertools::Itertools;
use memmap2::Mmap;
use nom::{
    bytes::complete::take_until,
    character::complete::{char, multispace0},
    combinator::{map, opt},
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
    /// The package version as a string
    pub version: String,
    /// The build string
    pub build: String,

    /// SHA256 hash of the package
    pub sha256: Option<Sha256Hash>,
    /// MD5 hash of the package, only if there is no SHA256 hash.
    pub md5: Option<Md5Hash>,
    /// Size of the package in bytes, only if there is no MD5 hash.
    pub size: Option<u64>,
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
        let mmap = unsafe { Mmap::map(&file)? };

        // Dynamic buffer sizing: start with 64KB, double until we find "files" field or can parse successfully
        let mut buffer_size = 65536; // Start with 64KB
        let max_buffer_size = 16 * 1024 * 1024; // Max 16MB

        let parsed = loop {
            let content_slice = if mmap.len() > buffer_size {
                &mmap[..buffer_size]
            } else {
                &mmap[..] // Use entire file if smaller than buffer
            };

            let content = std::str::from_utf8(content_slice)
                .map_err(|e| io::Error::other(format!("Invalid UTF-8: {e}")))?;

            // Try to parse with current buffer size
            match parse_minimal_json(content) {
                Ok(parsed) => break parsed,
                Err(_) if buffer_size < max_buffer_size && buffer_size < mmap.len() => {
                    // Double buffer size and retry
                    buffer_size *= 2;
                }
                Err(e) => {
                    return Err(io::Error::other(format!(
                        "Failed to parse JSON even with {}MB buffer. File size: {} bytes. Error: {}",
                        buffer_size / (1024 * 1024), mmap.len(), e
                    )));
                }
            }
        };

        // Apply the same logic as gjson parser: only one of sha256, md5, or size
        let (final_sha256, final_md5, final_size) = if parsed.sha256.is_some() {
            // If sha256 exists, use it and skip md5 and size
            (parsed.sha256, None, None)
        } else if parsed.md5.is_some() {
            // If no sha256 but md5 exists, use md5 and skip size
            (None, parsed.md5, None)
        } else {
            // If neither sha256 nor md5, use size
            (None, None, parsed.size)
        };

        // Apply the same logic as gjson parser: only parse python_site_packages_path for "python" packages
        let final_python_site_packages_path = if name.trim() == "python" {
            parsed.python_site_packages_path
        } else {
            None
        };

        #[allow(deprecated)]
        Ok(Self {
            name: name
                .parse::<PackageName>()
                .map_err(|e| format!("Could not parse package name: {e:#?}"))
                .map_err(io::Error::other)?,
            version: version.into(),
            build: build.into(),
            sha256: final_sha256,
            md5: final_md5,
            size: final_size,
            python_site_packages_path: final_python_site_packages_path,
            requested_specs: parsed.requested_specs,
            requested_spec: parsed.requested_spec,
        })
    }

    /// Convert to a partial `PackageRecord` for use in transaction computation.
    /// This creates a `PackageRecord` with only the essential fields filled in.
    pub fn to_package_record(&self) -> PackageRecord {
        let version = self
            .version
            .parse::<Version>()
            .unwrap_or_else(|_| Version::from_str(&self.version).unwrap());
        let version_with_source = VersionWithSource::from(version);

        PackageRecord {
            name: self.name.clone(),
            version: version_with_source,
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
    fn collect_minimal_from_prefix(prefix: &Path) -> Result<Vec<PrefixRecord>, io::Error>;
}

impl MinimalPrefixCollection for PrefixRecord {
    fn collect_minimal_from_prefix(prefix: &Path) -> Result<Vec<PrefixRecord>, io::Error> {
        let minimal_records = collect_minimal_prefix_records(prefix)?;

        // For now, we'll convert minimal records to full PrefixRecords with just the essential fields.
        // In the future, we could make Transaction work directly with SparsePrefixRecord.
        Ok(minimal_records
            .into_iter()
            .map(|minimal| {
                let package_record = minimal.to_package_record();
                let file_name = format!("{}-{}-{}.tar.bz2",
                    minimal.name.as_normalized(),
                    minimal.version,
                    minimal.build);
                #[allow(deprecated)]
                PrefixRecord {
                    repodata_record: crate::RepoDataRecord {
                        package_record,
                        file_name,
                        url: url::Url::parse("https://conda.anaconda.org/conda-forge/noarch/placeholder-1.0.0-0.tar.bz2").unwrap(),
                        channel: Some(String::new()),
                    },
                    package_tarball_full_path: None,
                    extracted_package_dir: None,
                    files: Vec::new(),
                    paths_data: crate::prefix_record::PrefixPaths::default(),
                    requested_spec: minimal.requested_spec.clone(),
                    requested_specs: minimal.requested_specs.clone(),
                    link: None,
                    installed_system_menus: Vec::new(),
                }
            })
            .collect())
    }
}

/// Struct to hold the parsed fields from JSON
#[derive(Debug, Default)]
struct ParsedFields {
    sha256: Option<Sha256Hash>,
    md5: Option<Md5Hash>,
    size: Option<u64>,
    python_site_packages_path: Option<String>,
    requested_specs: Vec<String>,
    requested_spec: Option<String>,
}

/// Parse minimal JSON fields using nom, stopping at "files" field
fn parse_minimal_json(input: &str) -> Result<ParsedFields, nom::Err<nom::error::Error<&str>>> {
    let (_, parsed) = parse_json_object(input)?;
    Ok(parsed)
}

/// Parse JSON object looking for specific fields
fn parse_json_object(input: &str) -> IResult<&str, ParsedFields> {
    let mut fields = ParsedFields::default();

    let (input, _) = preceded(multispace0, char('{')).parse(input)?;
    let (input, _) = multispace0(input)?;

    let (remaining, _) = parse_fields(input, &mut fields)?;

    Ok((remaining, fields))
}

/// Parse JSON fields until we find "files" or reach end
fn parse_fields<'a>(mut input: &'a str, fields: &mut ParsedFields) -> IResult<&'a str, ()> {
    loop {
        // Skip whitespace
        let (rest, _) = multispace0(input)?;
        input = rest;

        // Check for end of object
        if input.starts_with('}') {
            return Ok((input, ()));
        }

        // Parse field name
        let (rest, field_name) = parse_json_string(input)?;
        input = rest;

        // Check if we hit the "files" field - stop parsing here
        if field_name == "files" {
            return Ok((input, ()));
        }

        // Skip colon and whitespace
        let (rest, _) = preceded(multispace0, char(':')).parse(input)?;
        let (rest, _) = multispace0(rest)?;
        input = rest;

        // Parse field value based on field name
        let (rest, _) = parse_field_value(input, field_name, fields)?;
        input = rest;

        // Skip optional comma and whitespace
        let (rest, _) = multispace0(input)?;
        let (rest, _) = opt(char(',')).parse(rest)?;
        let (rest, _) = multispace0(rest)?;
        input = rest;
    }
}

/// Parse JSON string value (handles escaping)
fn parse_json_string(input: &str) -> IResult<&str, &str> {
    delimited(char('"'), take_until("\""), char('"')).parse(input)
}

/// Parse field value based on field name
fn parse_field_value<'a>(
    input: &'a str,
    field_name: &str,
    fields: &mut ParsedFields,
) -> IResult<&'a str, ()> {
    match field_name {
        "sha256" => {
            let (rest, value) = parse_json_string(input)?;
            if let Ok(bytes) = hex::decode(value) {
                if bytes.len() == 32 {
                    fields.sha256 = Some(Sha256Hash::from(
                        <[u8; 32]>::try_from(bytes.as_slice()).unwrap(),
                    ));
                }
            }
            Ok((rest, ()))
        }
        "md5" => {
            let (rest, value) = parse_json_string(input)?;
            if let Ok(bytes) = hex::decode(value) {
                if bytes.len() == 16 {
                    fields.md5 = Some(Md5Hash::from(
                        <[u8; 16]>::try_from(bytes.as_slice()).unwrap(),
                    ));
                }
            }
            Ok((rest, ()))
        }
        "size" => {
            let (rest, value) = take_until_comma_or_brace(input)?;
            fields.size = value.trim().parse().ok();
            Ok((rest, ()))
        }
        "python_site_packages_path" => {
            let (rest, value) = parse_json_string(input)?;
            fields.python_site_packages_path = Some(value.to_string());
            Ok((rest, ()))
        }
        "requested_spec" => {
            let (rest, value) = parse_json_string(input)?;
            fields.requested_spec = Some(value.to_string());
            Ok((rest, ()))
        }
        "requested_specs" => {
            let (rest, specs) = parse_json_array(input)?;
            fields.requested_specs = specs;
            Ok((rest, ()))
        }
        _ => {
            // Skip unknown fields
            let (rest, _) = skip_json_value(input)?;
            Ok((rest, ()))
        }
    }
}

/// Parse JSON array of strings
fn parse_json_array(input: &str) -> IResult<&str, Vec<String>> {
    delimited(
        preceded(multispace0, char('[')),
        separated_list0(
            preceded(multispace0, char(',')),
            preceded(multispace0, map(parse_json_string, ToString::to_string)),
        ),
        preceded(multispace0, char(']')),
    )
    .parse(input)
}

/// Skip any JSON value (string, number, object, array, boolean, null)
fn skip_json_value(input: &str) -> IResult<&str, ()> {
    let (rest, _) = multispace0(input)?;

    if rest.starts_with('"') {
        let (rest, _) = parse_json_string(rest)?;
        Ok((rest, ()))
    } else if rest.starts_with('[') {
        skip_json_array(rest)
    } else if rest.starts_with('{') {
        skip_json_object(rest)
    } else if rest.starts_with("true") || rest.starts_with("false") || rest.starts_with("null") {
        take_until_comma_or_brace(rest).map(|(rest, _)| (rest, ()))
    } else {
        // Number
        take_until_comma_or_brace(rest).map(|(rest, _)| (rest, ()))
    }
}

/// Skip JSON array
fn skip_json_array(input: &str) -> IResult<&str, ()> {
    let mut input = input;
    let (rest, _) = char('[').parse(input)?;
    input = rest;

    let mut depth = 1;
    while depth > 0 && !input.is_empty() {
        if input.starts_with('[') {
            depth += 1;
            input = &input[1..];
        } else if input.starts_with(']') {
            depth -= 1;
            input = &input[1..];
        } else {
            input = &input[1..];
        }
    }

    Ok((input, ()))
}

/// Skip JSON object
fn skip_json_object(input: &str) -> IResult<&str, ()> {
    let mut input = input;
    let (rest, _) = char('{').parse(input)?;
    input = rest;

    let mut depth = 1;
    while depth > 0 && !input.is_empty() {
        if input.starts_with('{') {
            depth += 1;
            input = &input[1..];
        } else if input.starts_with('}') {
            depth -= 1;
            input = &input[1..];
        } else {
            input = &input[1..];
        }
    }

    Ok((input, ()))
}

/// Take characters until comma, closing brace, or closing bracket
fn take_until_comma_or_brace(input: &str) -> IResult<&str, &str> {
    let mut end = 0;
    let chars: Vec<char> = input.chars().collect();

    for &ch in &chars {
        if ch == ',' || ch == '}' || ch == ']' {
            break;
        }
        end += ch.len_utf8();
    }

    Ok((&input[end..], &input[..end]))
}

#[allow(deprecated)]
#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    /// Test file with missing optional fields
    fn create_minimal_test_file() -> tempfile::TempDir {
        let temp_dir = tempfile::tempdir().unwrap();
        let file_path = temp_dir.path().join("minimal-package-0.1.0-build_1.json");

        let json_content = r#"{
  "name": "minimal-package",
  "version": "0.1.0",
  "build": "build_1",
  "build_number": 0,
  "depends": [],
  "requested_specs": [],
  "files": []
}"#;

        fs::write(&file_path, json_content).unwrap();
        temp_dir
    }

    /// Test file with only deprecated `requested_spec` field
    fn create_legacy_test_file() -> tempfile::TempDir {
        let temp_dir = tempfile::tempdir().unwrap();
        let file_path = temp_dir.path().join("legacy-package-2.0.0-abc123.json");

        let json_content = r#"{
  "name": "legacy-package",
  "version": "2.0.0",
  "build": "abc123",
  "build_number": 5,
  "depends": ["python"],
  "sha256": "fedcba0987654321fedcba0987654321fedcba0987654321fedcba0987654321",
  "size": 2097152,
  "requested_spec": "legacy-package ==2.0.0",
  "files": [
    "bin/legacy-tool"
  ]
}"#;

        fs::write(&file_path, json_content).unwrap();
        temp_dir
    }

    #[test]
    fn test_fast_parser_handles_missing_fields() {
        let temp_dir = create_minimal_test_file();
        let file_path = temp_dir.path().join("minimal-package-0.1.0-build_1.json");

        let result = MinimalPrefixRecord::from_path(&file_path).unwrap();

        assert_eq!(result.name.as_source(), "minimal-package");
        assert_eq!(result.version, "0.1.0");
        assert_eq!(result.build, "build_1");

        assert_eq!(result.sha256, None);
        assert_eq!(result.md5, None);
        assert_eq!(result.size, None);
        assert_eq!(result.python_site_packages_path, None);
        assert_eq!(result.requested_spec, None);

        assert!(result.requested_specs.is_empty());
    }

    #[test]
    fn test_parser_handles_legacy_requested_spec() {
        let temp_dir = create_legacy_test_file();
        let file_path = temp_dir.path().join("legacy-package-2.0.0-abc123.json");

        let result = MinimalPrefixRecord::from_path(&file_path).unwrap();

        assert_eq!(
            result.requested_spec,
            Some("legacy-package ==2.0.0".to_string())
        );
        assert!(result.requested_specs.is_empty());
    }

    #[test]
    fn test_parser_early_termination_at_files() {
        let temp_dir = tempfile::tempdir().unwrap();
        let file_path = temp_dir.path().join("early-term-test-1.0.0-build1.json");

        // Create JSON with "files" field in the middle, followed by more data
        let json_content = r#"{
  "name": "early-term-test",
  "version": "1.0.0",
  "build": "build1",
  "sha256": "abcdef1234567890abcdef1234567890abcdef1234567890abcdef1234567890",
  "requested_specs": ["early-term-test >=1.0"],
  "files": [
    "this should not be parsed",
    "neither should this"
  ],
  "this_field_after_files_should_be_ignored": "ignored_value",
  "another_ignored_field": 12345
}"#;

        fs::write(&file_path, json_content).unwrap();

        // The fast parser should work and ignore fields after "files"
        let result = MinimalPrefixRecord::from_path(&file_path).unwrap();

        assert_eq!(result.name.as_source(), "early-term-test");
        assert_eq!(result.version, "1.0.0");
        assert_eq!(result.build, "build1");
        assert!(result.sha256.is_some());
        assert_eq!(result.requested_specs, vec!["early-term-test >=1.0"]);
    }

    #[test]
    fn test_parse_large_file() {
        let temp_dir = tempfile::tempdir().unwrap();
        let file_path = temp_dir.path().join("size-test-1.0.0-build1.json");

        let json_content = format!(
            r#"{{
  "name": "size-test",
  "version": "1.0.0",
  "build": "build1",
  "sha256": "abcdef1234567890abcdef1234567890abcdef1234567890abcdef1234567890",
  "requested_specs": ["size-test >=1.0"],
  "files": [
    {}
  ]
}}"#,
            (0..5000)
                .map(|i| format!("\"large_file_{i}.txt\""))
                .collect::<Vec<_>>()
                .join(",\n    ")
        );

        fs::write(&file_path, json_content).unwrap();

        let result = MinimalPrefixRecord::from_path(&file_path).unwrap();

        assert_eq!(result.name.as_source(), "size-test");
        assert_eq!(result.version, "1.0.0");
        assert_eq!(result.build, "build1");
        assert!(result.sha256.is_some());
        assert_eq!(result.requested_specs, vec!["size-test >=1.0"]);
    }

    #[test]
    fn test_parser_buffer_doubling() {
        let temp_dir = tempfile::tempdir().unwrap();
        let file_path = temp_dir.path().join("buffer-test-1.0.0-build1.json");

        let mut json_content = r#"{"#.to_string();

        json_content.push_str(r#""large_padding": ""#);
        json_content.push_str(&"x".repeat(70000)); // 70KB of padding
        json_content.push_str(r#"","#);

        json_content.push_str(
            r#"
  "name": "buffer-test",
  "version": "1.0.0",
  "build": "build1",
  "sha256": "1111222233334444555566667777888899990000aaaabbbbccccddddeeeeffff",
  "requested_specs": ["buffer-test >=1.0"],
  "files": [
    "file1.txt",
    "file2.txt"
  ]
}"#,
        );

        fs::write(&file_path, &json_content).unwrap();

        assert!(
            json_content.len() > 65536,
            "Test file should be larger than 64KB"
        );

        let result = MinimalPrefixRecord::from_path(&file_path).unwrap();

        assert_eq!(result.name.as_source(), "buffer-test");
        assert_eq!(result.version, "1.0.0");
        assert_eq!(result.build, "build1");
        assert!(result.sha256.is_some());
        assert_eq!(result.requested_specs, vec!["buffer-test >=1.0"]);
    }
}
