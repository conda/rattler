/// VersionBumpType is used to specify the type of bump to perform on a version.
// #[derive(Default)]
pub enum VersionBumpType {
    /// Bump the major version number.
    Major,
    /// Bump the minor version number.
    Minor,
    /// Bump the patch version number.
    Patch,
    /// Bump the last  version number.
    // #[default]
    Last,
}
