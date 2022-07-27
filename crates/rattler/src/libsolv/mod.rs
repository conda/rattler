use std::ffi::CString;

mod ffi;
mod pool;
mod queue;
mod repo;

/// Convenience method to convert from a string reference to a CString
fn c_string<T: AsRef<str>>(str: T) -> CString {
    CString::new(str.as_ref()).expect("could never be null because of trait-bound")
}

#[cfg(test)]
mod test {
    use crate::libsolv::pool::Pool;

    #[test]
    fn test_conda_read_repodata() {
        let json_file = format!(
            "{}/{}",
            env!("CARGO_MANIFEST_DIR"),
            "resources/conda_forge_noarch_repodata.json"
        );
        let mut pool = Pool::default();
        let mut repo = pool.create_repo("conda-forge");
        repo.add_conda_json(json_file)
            .expect("could not add repodata to Repo");
    }
}
