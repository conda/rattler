//! Defines the [`fetch_repodata`] function which reads repodata information from disk.

use crate::repo_data::fetch::{DoneState, RepoDataRequestState, RequestRepoDataError};
use rattler_conda_types::RepoData;
use std::{
    fs::OpenOptions,
    io::{BufReader, Read},
    path::Path,
};

/// Read [`RepoData`] from disk. No caching is performed since the data already resides on disk
/// anyway.
///
/// The `listener` parameter allows following the progress of the request through its various
/// stages. See [`RepoDataRequestState`] for the various stages a request can go through. As reading
/// repodata can take several seconds the `listener` can be used to show some visual feedback to the
/// user.
pub async fn fetch_repodata(
    path: &Path,
    listener: &mut impl FnMut(RepoDataRequestState),
) -> Result<(RepoData, DoneState), RequestRepoDataError> {
    // Read the entire file to memory. This does probably cost a lot more memory, but
    // deserialization is much (~10x) faster. Since this might take some time, we run this in a
    // in a separate background task to ensure we don't unnecessarily block the current thread.
    let path = path.to_owned();
    let bytes = tokio::task::spawn_blocking(move || -> Result<Vec<u8>, RequestRepoDataError> {
        let file = OpenOptions::new().read(true).write(true).open(&path)?;
        let mut bytes = Vec::new();
        BufReader::new(file).read_to_end(&mut bytes)?;
        Ok(bytes)
    })
    .await??;

    // Now that we have all the data in memory we can deserialize the content using `serde`. Since
    // repodata information can be quite huge we run the deserialization in a separate background
    // task to ensure we don't block the current thread.
    listener(RepoDataRequestState::Deserializing);
    let repodata = tokio::task::spawn_blocking(move || serde_json::from_slice(&bytes)).await??;

    // No cache is used, so there was definitely a cache miss.
    Ok((repodata, DoneState { cache_miss: true }))
}

#[cfg(test)]
mod test {
    use super::fetch_repodata;
    use crate::get_test_data_dir;

    #[tokio::test]
    async fn test_fetch_file() {
        let subdir_path = get_test_data_dir().join("channels/empty/noarch/repodata.json");
        let _ = fetch_repodata(&subdir_path, &mut |_| {}).await.unwrap();
    }
}
