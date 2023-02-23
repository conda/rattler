use std::{
    ffi::OsStr,
    fs,
    path::{Path, PathBuf},
};

use indexmap::IndexMap;

#[derive(Copy, Clone, Debug)]
pub enum ShellType {
    Bash,
    Zsh,
    Fish,
    Powershell,
    CmdExe,
}

#[derive(Copy, Clone, Debug)]
pub enum OperatingSystem {
    Windows,
    Unix,
}

/// A struct that holds values for the activation and deactivation
/// process of an environment, e.g. activation scripts to execute or environment variables to set.
pub struct Activator {
    /// The path to the root of the conda environment
    pub target_prefix: PathBuf,

    /// The type of shell that is being activated
    pub shell_type: ShellType,

    /// The new PATH variable after activation
    pub paths: Vec<PathBuf>,

    /// A list of scripts to run when activating the environment
    pub activation_scripts: Vec<PathBuf>,

    /// A list of scripts to run when deactivating the environment
    pub deactivation_scripts: Vec<PathBuf>,

    /// A list of environment variables to set when activating the environment
    pub env_vars: IndexMap<String, String>,
}

impl ShellType {
    fn suffix(&self) -> &'static OsStr {
        match self {
            ShellType::Bash => OsStr::new("sh"),
            ShellType::Zsh => OsStr::new("zsh"),
            ShellType::Fish => OsStr::new("fish"),
            ShellType::Powershell => OsStr::new("ps1"),
            ShellType::CmdExe => OsStr::new("bat"),
        }
    }
}

fn collect_scripts(path: &PathBuf, shell_type: &ShellType) -> anyhow::Result<Vec<PathBuf>> {
    if !path.exists() {
        return Ok(vec![]);
    }

    let paths = fs::read_dir(path).unwrap();

    let mut scripts = paths
        .into_iter()
        .filter(|r| r.is_ok())
        .map(|r| r.unwrap().path())
        .filter(|path| path.is_file() && path.extension() == Some(shell_type.suffix()))
        .collect::<Vec<_>>();

    scripts.sort();

    Ok(scripts)
}

fn collect_env_vars(prefix: &Path) -> anyhow::Result<IndexMap<String, String>> {
    let state_file = prefix.join("conda-meta/state");
    let pkg_env_var_dir = prefix.join("etc/conda/env_vars.d");
    let mut env_vars = IndexMap::new();

    if pkg_env_var_dir.exists() {
        let env_var_files = pkg_env_var_dir.read_dir().unwrap();
        let mut env_var_files = env_var_files
            .into_iter()
            .filter(|r| r.is_ok())
            .map(|r| r.unwrap().path())
            .filter(|path| path.is_file())
            .collect::<Vec<_>>();

        env_var_files.sort();

        let env_var_json: Vec<serde_json::Value> = env_var_files
            .iter()
            .map(|path| fs::read_to_string(path).unwrap())
            .map(|json| serde_json::from_str(&json).unwrap())
            .collect();

        for env_var_json in env_var_json {
            for (key, value) in env_var_json.as_object().unwrap() {
                println!("{}: {}", key, value.as_str().unwrap());
                env_vars.insert(key.to_uppercase(), value.as_str().unwrap().to_string());
            }
        }
    }

    if state_file.exists() {
        let state_json = fs::read_to_string(state_file).unwrap();

        // load json but preserve the order of dicts - for this we use the serde preserve_order feature
        let state_json: serde_json::Value = serde_json::from_str(&state_json).unwrap();
        let state_env_vars = state_json["env_vars"].as_object().unwrap();

        for (key, value) in state_env_vars {
            if env_vars.contains_key(key) {
                println!("{}: {} (overwritten)", key, value.as_str().unwrap());
            } else {
                println!("{}: {}", key, value.as_str().unwrap());
            }
            env_vars.insert(key.to_uppercase(), value.as_str().unwrap().to_string());
        }
    }
    Ok(env_vars)
}

fn prefix_path_entries(
    prefix: &Path,
    operating_system: &OperatingSystem,
) -> anyhow::Result<Vec<PathBuf>> {
    let new_paths: Vec<PathBuf> = match operating_system {
        OperatingSystem::Windows => {
            vec![
                prefix.to_path_buf(),
                prefix.join("Library/mingw-w64/bin"),
                prefix.join("Library/usr/bin"),
                prefix.join("Library/bin"),
                prefix.join("Scripts"),
                prefix.join("bin"),
            ]
        }
        OperatingSystem::Unix => {
            vec![prefix.join("bin")]
        }
    };
    Ok(new_paths)
}

impl Activator {
    pub fn from_path(
        path: &Path,
        shell_type: &ShellType,
        operating_system: &OperatingSystem,
    ) -> Result<Activator, String> {
        let activation_scripts =
            collect_scripts(&path.to_path_buf().join("etc/conda/activate.d"), shell_type)
                .expect("Couldn't collect scripts");
        let deactivation_scripts = collect_scripts(
            &path.to_path_buf().join("etc/conda/deactivate.d"),
            shell_type,
        )
        .expect("Couldn't collect scripts");

        let env_vars = collect_env_vars(path).expect("Couldn't collect env vars");
        let paths = prefix_path_entries(path, operating_system).expect("Couldn't add to path");
        Ok(Activator {
            target_prefix: path.to_path_buf(),
            shell_type: *shell_type,
            paths,
            activation_scripts,
            deactivation_scripts,
            env_vars,
        })
    }
}

#[cfg(test)]
mod tests {
    use std::str::FromStr;

    use super::*;
    use tempdir::TempDir;

    #[test]
    fn test_collect_scripts() {
        let tdir = TempDir::new("test").unwrap();

        let path = tdir.path().join("etc/conda/activate.d/");
        fs::create_dir_all(&path).unwrap();

        let script1 = path.join("script1.sh");
        let script2 = path.join("aaa.sh");
        let script3 = path.join("xxx.sh");

        fs::write(&script1, "").unwrap();
        fs::write(&script2, "").unwrap();
        fs::write(&script3, "").unwrap();

        let shell_type = ShellType::Bash;

        let scripts = collect_scripts(&path, &shell_type).unwrap();
        assert_eq!(scripts.len(), 3);
        assert_eq!(scripts[0], script2);
        assert_eq!(scripts[1], script1);
        assert_eq!(scripts[2], script3);

        let activator = Activator::from_path(
            &tdir.path().to_path_buf(),
            &shell_type,
            &OperatingSystem::Unix,
        )
        .unwrap();
        assert_eq!(activator.activation_scripts.len(), 3);
        assert_eq!(activator.activation_scripts[0], script2);
        assert_eq!(activator.activation_scripts[1], script1);
        assert_eq!(activator.activation_scripts[2], script3);
    }

    #[test]
    fn test_collect_env_vars() {
        let tdir = TempDir::new("test").unwrap();
        let path = tdir.path().join("conda-meta/state");
        fs::create_dir_all(path.parent().unwrap()).unwrap();

        let quotes = r#"{"env_vars": {"Hallo": "myval", "TEST": "itsatest", "AAA": "abcdef"}}"#;
        fs::write(&path, quotes).unwrap();

        let env_vars = collect_env_vars(&tdir.path().to_path_buf()).unwrap();
        assert_eq!(env_vars.len(), 3);

        assert_eq!(env_vars["HALLO"], "myval");
        assert_eq!(env_vars["TEST"], "itsatest");
        assert_eq!(env_vars["AAA"], "abcdef");
    }

    #[test]
    fn test_collect_env_vars_with_directory() {
        let tdir = TempDir::new("test").unwrap();
        let state_path = tdir.path().join("conda-meta/state");
        fs::create_dir_all(state_path.parent().unwrap()).unwrap();

        let content_pkg_1 = r#"{"VAR1": "someval", "TEST": "pkg1-test", "III": "super"}"#;
        let content_pkg_2 = r#"{"VAR1": "overwrite1", "TEST2": "pkg2-test"}"#;

        let env_var_d = tdir.path().join("etc/conda/env_vars.d");
        fs::create_dir_all(&env_var_d).expect("Could not create env vars directory");

        let pkg1 = env_var_d.join("pkg1.json");
        let pkg2 = env_var_d.join("pkg2.json");

        fs::write(&pkg1, content_pkg_1).expect("could not write file");
        fs::write(&pkg2, content_pkg_2).expect("could not write file");

        let quotes = r#"{"env_vars": {"Hallo": "myval", "TEST": "itsatest", "AAA": "abcdef"}}"#;
        fs::write(&state_path, quotes).unwrap();

        let env_vars =
            collect_env_vars(&tdir.path().to_path_buf()).expect("Could not load env vars");
        assert_eq!(env_vars.len(), 6);

        assert_eq!(env_vars["VAR1"], "overwrite1");
        assert_eq!(env_vars["TEST"], "itsatest");
        assert_eq!(env_vars["III"], "super");
        assert_eq!(env_vars["TEST2"], "pkg2-test");
        assert_eq!(env_vars["HALLO"], "myval");
        assert_eq!(env_vars["AAA"], "abcdef");

        // assert order of keys
        let mut keys = env_vars.keys();
        let key_vec = vec![
            "VAR1", // overwritten - should this be sorted down?
            "TEST", "III", "TEST2", "HALLO", "AAA",
        ];

        for key in key_vec {
            assert_eq!(keys.next().unwrap(), key);
        }
    }

    #[test]
    fn test_add_to_path() {
        let prefix = PathBuf::from_str("/opt/conda").unwrap();
        let new_paths = prefix_path_entries(&prefix, &OperatingSystem::Unix);
        println!("{:?}", new_paths);
        assert_eq!(new_paths.unwrap().len(), 1);
    }
}
