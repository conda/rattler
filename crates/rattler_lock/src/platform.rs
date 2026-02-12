/// An error that can occur when parsing a platform from a string.
#[derive(Debug, Clone, thiserror::Error, Eq, PartialEq)]
#[allow(missing_docs)]
pub enum ParsePlatformError {
    #[error("failed to parse '{0}' as a PlatformName")]
    ParsePlatformNameError(String),

    #[error("failed to parse '{0}' as a Subdir")]
    ParseSubdirError(#[from] rattler_conda_types::ParsePlatformError),
}

/// A valid name for a platform
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct PlatformName(String);

impl From<&rattler_conda_types::Platform> for PlatformName {
    fn from(value: &rattler_conda_types::Platform) -> Self {
        Self(value.to_string())
    }
}

impl std::convert::TryFrom<String> for PlatformName {
    type Error = ParsePlatformError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        if value
            .chars()
            .all(|c| c.is_alphanumeric() || c == '_' || c == '-')
        {
            Ok(Self(value))
        } else {
            Err(ParsePlatformError::ParsePlatformNameError(value))
        }
    }
}

impl std::convert::TryFrom<&str> for PlatformName {
    type Error = ParsePlatformError;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        Self::try_from(value.to_string())
    }
}

impl std::fmt::Display for PlatformName {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl std::str::FromStr for PlatformName {
    type Err = ParsePlatformError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::try_from(value)
    }
}

impl std::ops::Deref for PlatformName {
    type Target = String;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

/// A reference to a platform in a lock file.
///
/// This type provides access to platform-specific information stored in the
/// lock file, including the platform name, the underlying conda subdir, and
/// any virtual packages associated with the platform.
#[derive(Clone, Copy)]
pub struct Platform<'lock> {
    pub(crate) index: usize,
    pub(crate) lock_file_inner: &'lock crate::LockFileInner,
}

impl<'lock> std::hash::Hash for Platform<'lock> {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.index.hash(state);
        std::ptr::from_ref(self.lock_file_inner).hash(state);
    }
}

impl<'lock> PartialEq for Platform<'lock> {
    fn eq(&self, other: &Self) -> bool {
        self.index == other.index
            && std::ptr::from_ref(self.lock_file_inner) == std::ptr::from_ref(other.lock_file_inner)
    }
}

impl<'lock> Eq for Platform<'lock> {}

impl<'lock> Platform<'lock> {
    pub(crate) fn new(lock_file: &'lock crate::LockFileInner, index: usize) -> Self {
        Self {
            index,
            lock_file_inner: lock_file,
        }
    }

    fn data(&self) -> &PlatformData {
        self.lock_file_inner
            .platforms
            .get(self.index)
            .expect("Platform is always valid")
    }

    /// Returns the name of the platform (e.g., "linux-64", "osx-arm64").
    pub fn name(&self) -> &PlatformName {
        &self.data().name
    }

    /// Returns the underlying conda subdir/platform.
    pub fn subdir(&self) -> rattler_conda_types::Platform {
        self.data().subdir
    }

    /// Returns the list of virtual packages for this platform.
    pub fn virtual_packages(&self) -> &[String] {
        &self.data().virtual_packages
    }

    /// Creates an owned version of this platform reference.
    pub fn to_owned(self, lock_file: &crate::LockFile) -> OwnedPlatform {
        OwnedPlatform {
            lock_file: lock_file.clone(),
            index: self.index,
        }
    }
}

/// An owned version of a [`Platform`].
///
/// Use [`OwnedPlatform::as_ref`] to get a reference to the platform data.
#[derive(Clone)]
pub struct OwnedPlatform {
    lock_file: crate::LockFile,
    index: usize,
}

impl OwnedPlatform {
    /// Returns a reference to the platform data.
    pub fn as_ref(&self) -> Platform<'_> {
        Platform::new(&self.lock_file.inner, self.index)
    }

    /// Returns the lock-file this platform is part of.
    pub fn lock_file(&self) -> crate::LockFile {
        self.lock_file.clone()
    }
}

pub(crate) fn find_index_by_name(platforms: &[PlatformData], name: &str) -> Option<usize> {
    platforms.iter().position(|p| p.name.as_str() == name)
}

/// Represents a package with platform-specific information.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PlatformData {
    /// The name of the platform.
    pub name: PlatformName,
    /// The subdir of the platform.
    pub subdir: rattler_conda_types::Platform,
    /// The list of virtual conda packages.
    pub virtual_packages: Vec<String>,
}

impl std::hash::Hash for PlatformData {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.name.hash(state);
    }
}

pub(crate) struct PlatformIterator<'lock> {
    indices: Vec<Platform<'lock>>,
    current_index_pos: usize,
}

impl<'lock> PlatformIterator<'lock> {
    pub fn new(indices: Vec<Platform<'lock>>) -> Self {
        Self {
            indices,
            current_index_pos: 0,
        }
    }
}

impl<'lock> Iterator for PlatformIterator<'lock> {
    type Item = Platform<'lock>;

    fn next(&mut self) -> Option<Self::Item> {
        let current = self.current_index_pos;
        self.current_index_pos = current.saturating_add(1);
        self.indices.get(current).copied()
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        let remaining = self.indices.len().saturating_sub(self.current_index_pos);
        (remaining, Some(remaining))
    }
}

impl<'lock> ExactSizeIterator for PlatformIterator<'lock> {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_platform_name() {
        assert!(PlatformName::try_from(String::from("test_1-2_3")).is_ok());
        assert!(PlatformName::try_from(String::from("linux-64")).is_ok());
        assert!(PlatformName::try_from(String::from("linux 64")).is_err());
        assert!(PlatformName::try_from(String::from("linux+64")).is_err());
    }
}
