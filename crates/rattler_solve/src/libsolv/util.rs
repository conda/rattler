#[derive(Copy, Clone, Ord, PartialEq, PartialOrd, Eq)]
pub enum PackageExtension {
    TarBz2,
    Conda,
}

/// Given a package filename, extracts the filename and the extension if the extension is a known
/// package extension.
pub fn extract_known_filename_extension(filename: &str) -> Option<(&str, PackageExtension)> {
    if let Some(filename) = filename.strip_suffix(".conda") {
        Some((filename, PackageExtension::Conda))
    } else {
        filename
            .strip_suffix(".tar.bz2")
            .map(|filename| (filename, PackageExtension::TarBz2))
    }
}
