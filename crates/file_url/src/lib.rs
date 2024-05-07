//! The URL crate parses `file://` URLs differently on Windows and other operating systems.
//! This crates provides functionality that tries to parse a `file://` URL as a path on all operating
//! systems. This is useful when you want to convert a `file://` URL to a path and vice versa.

use itertools::Itertools;
use percent_encoding::{percent_decode, percent_encode, AsciiSet, CONTROLS};
use std::fmt::Write;
use std::path::{Path, PathBuf};
use std::str::FromStr;
use url::{Host, Url};

/// Returns true if the specified segment is considered to be a Windows drive letter segment.
/// E.g. the segment `C:` or `C%3A` would be considered a drive letter segment.
fn is_windows_drive_letter_segment(segment: &str) -> Option<String> {
    // Segment is a simple drive letter: X:
    if let Some((drive_letter, ':')) = segment.chars().collect_tuple() {
        if drive_letter.is_ascii_alphabetic() {
            return Some(format!("{drive_letter}:\\"));
        }
    }

    // Segment is a simple drive letter but the colon is percent escaped: E.g. X%3A
    if let Some((drive_letter, '%', '3', 'a' | 'A')) = segment.chars().collect_tuple() {
        if drive_letter.is_ascii_alphabetic() {
            return Some(format!("{drive_letter}:\\"));
        }
    }

    None
}

/// Tries to convert a `file://` based URL to a path.
///
/// We assume that any passed URL that represents a path is an absolute path.
///
/// [`Url::to_file_path`] has a different code path for Windows and other operating systems, this
/// can cause URLs to parse perfectly fine on Windows, but fail to parse on Linux. This function
/// tries to parse the URL as a path on all operating systems.
pub fn url_to_path(url: &Url) -> Option<PathBuf> {
    if url.scheme() != "file" {
        return None;
    }

    let mut segments = url.path_segments()?;
    let host = match url.host() {
        None | Some(Host::Domain("localhost")) => None,
        Some(host) => Some(host),
    };

    let (mut path, seperator) = if let Some(host) = host {
        // A host is only present for Windows UNC paths
        (format!("\\\\{host}\\"), "\\")
    } else {
        let first = segments.next()?;
        if first.starts_with('.') {
            // Relative file paths are not supported
            return None;
        }

        match is_windows_drive_letter_segment(first) {
            Some(drive_letter) => (drive_letter, "\\"),
            None => (format!("/{first}/"), "/"),
        }
    };

    for (idx, segment) in segments.enumerate() {
        if idx > 0 {
            path.push_str(seperator);
        }
        match String::from_utf8(percent_decode(segment.as_bytes()).collect()) {
            Ok(s) => path.push_str(&s),
            _ => return None,
        }
    }

    Some(PathBuf::from(path))
}

const FRAGMENT: &AsciiSet = &CONTROLS.add(b' ').add(b'"').add(b'<').add(b'>').add(b'`');
const PATH: &AsciiSet = &FRAGMENT.add(b'#').add(b'?').add(b'{').add(b'}');
pub(crate) const PATH_SEGMENT: &AsciiSet = &PATH.add(b'/').add(b'%');

/// Whether the scheme is file:, the path has a single segment, and that segment
/// is a Windows drive letter
#[inline]
pub fn is_windows_drive_letter(segment: &str) -> bool {
    segment.len() == 2 && starts_with_windows_drive_letter(segment)
}

fn starts_with_windows_drive_letter(s: &str) -> bool {
    s.len() >= 2
        && (s.as_bytes()[0] as char).is_ascii_alphabetic()
        && matches!(s.as_bytes()[1], b':' | b'|')
        && (s.len() == 2 || matches!(s.as_bytes()[2], b'/' | b'\\' | b'?' | b'#'))
}

fn path_to_url(path: &Path) -> Result<String, NotAnAbsolutePath> {
    use std::path::{Component, Prefix};
    let mut components = path.components();

    let mut result = String::from("file://");
    let host_start = result.len() + 1;

    let root = components.next();
    match root {
        Some(Component::Prefix(ref p)) => match p.kind() {
            Prefix::Disk(letter) | Prefix::VerbatimDisk(letter) => {
                // host_end = to_u32(serialization.len()).unwrap();
                // host_internal = HostInternal::None;
                result.push('/');
                result.push(letter as char);
                result.push(':');
            }
            Prefix::UNC(server, share) | Prefix::VerbatimUNC(server, share) => {
                let host = Host::parse(server.to_str().ok_or(NotAnAbsolutePath)?).map_err(|_err| NotAnAbsolutePath)?;
                write!(result, "{host}").unwrap();
                // host_end = to_u32(serialization.len()).unwrap();
                // host_internal = host.into();
                result.push('/');
                let share = share.to_str().ok_or(NotAnAbsolutePath)?;
                result.extend(percent_encode(share.as_bytes(), PATH_SEGMENT));
            }
            _ => return Err(NotAnAbsolutePath),
        },
        Some(Component::RootDir) => {}
        _ => return Err(NotAnAbsolutePath),
    }

    let mut path_only_has_prefix = true;
    for component in components {
        if component == Component::RootDir {
            continue;
        }

        path_only_has_prefix = false;
        // FIXME: somehow work with non-unicode?
        let component = component.as_os_str().to_str().ok_or(NotAnAbsolutePath)?;

        result.push('/');
        result.extend(percent_encode(component.as_bytes(), PATH_SEGMENT));
    }

    // A windows drive letter must end with a slash.
    if result.len() > host_start
        && is_windows_drive_letter(&result[host_start..])
        && path_only_has_prefix
    {
        result.push('/');
    }

    Ok(result)
}

pub struct NotAnAbsolutePath;

pub fn file_path_to_url(path: &Path) -> Result<Url, NotAnAbsolutePath> {
    let url = path_to_url(path)?;
    Ok(Url::from_str(&url).expect("url string must be a valid url"))
}

pub fn directory_path_to_url(path: &Path) -> Result<Url, NotAnAbsolutePath> {
    let mut url = path_to_url(path)?;
    if !url.ends_with('/') {
        url.push('/');
    }
    Ok(Url::from_str(&url).expect("url string must be a valid url"))
}

#[cfg(test)]
mod tests {
    use rstest::rstest;
    use std::path::PathBuf;
    use url::Url;

    #[rstest]
    #[case("file:///home/bob/test-file.txt", Some("/home/bob/test-file.txt"))]
    #[case("file:///C:/Test/Foo.txt", Some("C:\\Test\\Foo.txt"))]
    #[case("file:///c:/temp/test-file.txt", Some("c:\\temp\\test-file.txt"))]
    #[case("file:///c:\\temp\\test-file.txt", Some("c:\\temp\\test-file.txt"))]
    // Percent encoding
    #[case("file:///foo/ba%20r", Some("/foo/ba r"))]
    #[case("file:///C%3A/Test/Foo.txt", Some("C:\\Test\\Foo.txt"))]
    // Non file URLs
    #[case("http://example.com", None)]
    fn test_url_to_path(#[case] url: &str, #[case] expected: Option<&str>) {
        let url = url.parse::<Url>().unwrap();
        let expected = expected.map(PathBuf::from);
        assert_eq!(super::url_to_path(&url), expected);
    }

    #[rstest]
    #[case::win_drive("C:/", Some("file:///C:/"))]
    #[case::unix_path("/root", Some("file:///root"))]
    #[case::not_absolute("root", None)]
    #[case::win_share("//servername/path", Some("file://servername/path"))]
    #[case::dos_device_path("\\\\?\\C:\\Test\\Foo.txt", Some("file:///C:/Test/Foo.txt"))]
    #[case::unsupported_guid_volumes(
        "\\\\.\\Volume{b75e2c83-0000-0000-0000-602f00000000}\\Test\\Foo.txt",
        None
    )]
    #[case::ip_address(
        "\\127.0.0.1\\c$\\temp\\test-file.txt",
        Some("file:///127.0.0.1/c$/temp/test-file.txt")
    )]
    #[case::percent_encoding("//foo/ba r", Some("file://foo/ba%20r"))]
    fn test_file_path_to_url(#[case] path: &str, #[case] expected: Option<&str>) {
        let path = PathBuf::from(path);
        let expected = expected.map(|s| s.to_string());
        assert_eq!(
            super::file_path_to_url(&path).map(|u| u.to_string()).ok(),
            expected
        );
    }
}
