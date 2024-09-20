use std::path::{Path, PathBuf};

pub mod unix_lex;

pub fn menuinst_data_paths(prefix: &Path) -> Vec<PathBuf> {
    vec![prefix.join("share/menuinst")]
}

pub fn quote_args(args: &[String]) -> Vec<String> {
    args.iter().map(|arg| format!(r#""{arg}""#)).collect()
}
