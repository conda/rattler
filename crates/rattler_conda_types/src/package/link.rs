use std::path::Path;

use serde::{Deserialize, Serialize};

use super::{EntryPoint, PackageFile};

#[derive(Serialize, Clone, Debug, Deserialize)]
struct NoarchPython {
    entry_points: Vec<EntryPoint>,
}

#[derive(Serialize, Clone, Debug, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
enum Noarch {
    Python(NoarchPython),
}

/// A representation of the `link.json` file found in noarch package archives.
///
/// The `link.json` file contains information about entrypoints that need to be installed for the package.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct LinkJson {
    noarch: Noarch,

    /// The version of the package metadata file
    pub package_metadata_version: usize,
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

    #[test]
    fn test_link_json() {
        let test_file = &crate::get_test_data_dir().join("link-json/jupyterlab-link.json");
        let link_json: LinkJson =
            serde_json::from_reader(std::fs::File::open(test_file).unwrap()).unwrap();
        insta::assert_yaml_snapshot!(link_json);
    }
}
