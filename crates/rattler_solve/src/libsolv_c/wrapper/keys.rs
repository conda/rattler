// The constants below are taken from
// https://github.com/openSUSE/libsolv/blob/67aaf74844c532129ec8d7c8a7be4209ee4ef78d/src/knownid.h
//
// We have only copied those that are interesting for rattler

pub const SOLVABLE_LICENSE: &str = "solvable:license";
pub const SOLVABLE_BUILDTIME: &str = "solvable:buildtime";
pub const SOLVABLE_DOWNLOADSIZE: &str = "solvable:downloadsize";
pub const SOLVABLE_CHECKSUM: &str = "solvable:checksum";
pub const SOLVABLE_PKGID: &str = "solvable:pkgid";
pub const SOLVABLE_BUILDFLAVOR: &str = "solvable:buildflavor";
pub const SOLVABLE_BUILDVERSION: &str = "solvable:buildversion";
pub const REPOKEY_TYPE_MD5: &str = "repokey:type:md5";
pub const REPOKEY_TYPE_SHA256: &str = "repokey:type:sha256";
pub const SOLVABLE_CONSTRAINS: &str = "solvable:constrains";
pub const SOLVABLE_TRACK_FEATURES: &str = "solvable:track_features";
