use std::path::{Path, PathBuf};

pub mod unix_lex;

pub fn menuinst_data_paths(prefix: &Path) -> Vec<PathBuf> {
    vec![prefix.join("share/menuinst")]
}

pub fn quote_args<I, S>(args: I) -> Vec<String>
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    args.into_iter()
        .map(|arg| format!(r#""{}""#, arg.as_ref()))
        .collect()
}
