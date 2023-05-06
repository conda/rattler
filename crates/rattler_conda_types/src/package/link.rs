use std::path::Path;

use rattler_macros::sorted;
use serde::{Deserialize, Serialize};

use super::{EntryPoint, PackageFile};

/// Describes python noarch specific entry points
#[derive(Serialize, Clone, Debug, Deserialize)]
pub struct PythonEntryPoints {
    /// A list of commands that should execute certain python commands.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub entry_points: Vec<EntryPoint>,
}

/// Links for specific types of noarch packages.
#[derive(Serialize, Clone, Debug, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum NoArchLinks {
    /// Python noarch specific entry points.
    Python(PythonEntryPoints),

    /// Generic variant (doesn't have any special entry points)
    Generic,
}

/// A representation of the `link.json` file found in noarch package archives.
///
/// The `link.json` file contains information about entrypoints that need to be installed for the package.
#[sorted]
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct LinkJson {
    /// Links for specific noarch packages
    pub noarch: NoArchLinks,

    /// The version of the package metadata file
    pub package_metadata_version: u64,
}

impl PackageFile for LinkJson {
    fn package_path() -> &'static Path {
        Path::new("info/link.json")
    }

    fn from_str(str: &str) -> Result<Self, std::io::Error> {
        serde_json::from_str(str).map_err(Into::into)
    }
}

#[cfg(test)]
mod test {
    use super::LinkJson;
    use rstest::rstest;

    #[rstest]
    #[case::jupyterlab("link-json/jupyterlab-link.json")]
    #[case::setuptools("link-json/setuptools-link.json")]
    #[case::tzdata("link-json/tzdata-link.json")]
    fn test_link_json(#[case] path: &str) {
        let test_file = &crate::get_test_data_dir().join(path);
        let link_json: LinkJson =
            serde_json::from_reader(std::fs::File::open(test_file).unwrap()).unwrap();
        insta::assert_yaml_snapshot!(path, link_json);
    }
}
