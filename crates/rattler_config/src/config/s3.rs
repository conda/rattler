use serde::{Deserialize, Serialize};
use url::Url;

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub struct S3Options {
    /// S3 endpoint URL
    pub endpoint_url: Url,

    /// The name of the S3 region
    pub region: String,

    /// Force path style URLs instead of subdomain style
    pub force_path_style: bool,
}
