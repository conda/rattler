use fxhash::{FxHashMap, FxHashSet};
use serde::Deserialize;

#[derive(Debug, Deserialize)]
pub struct Repodata {
    pub info: Option<ChannelInfo>,
    pub packages: FxHashMap<String, Record>,
    #[serde(default)]
    pub removed: FxHashSet<String>,
    pub repodata_version: Option<usize>,
}

#[derive(Debug, Deserialize)]
pub struct ChannelInfo {
    pub subdir: String,
}

#[derive(Debug, Deserialize, Default)]
pub struct Record {
    pub name: String,
    pub build: String,
    pub build_number: usize,
    #[serde(default)]
    pub depends: Vec<String>,
    #[serde(default)]
    pub constrains: Vec<String>,
    pub license: Option<String>,
    pub license_family: Option<String>,
    pub md5: String,
    pub sha256: Option<String>,
    pub size: usize,
    pub subdir: String,
    pub timestamp: Option<usize>,
    pub version: String,
}
