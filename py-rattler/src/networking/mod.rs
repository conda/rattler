use futures::future::try_join_all;
use pyo3::{pyfunction, types::PyTuple, Bound, Py, PyAny, PyResult, Python, ToPyObject};
use pyo3_async_runtimes::tokio::future_into_py;

use rattler_repodata_gateway::fetch::{
    fetch_repo_data, CachedRepoData, FetchRepoDataError, FetchRepoDataOptions,
};
use url::Url;

use std::{path::PathBuf, str::FromStr, sync::Arc};

use crate::{
    channel::PyChannel, error::PyRattlerError, platform::PyPlatform,
    repo_data::sparse::PySparseRepoData,
};
use client::PyClientWithMiddleware;
use rattler_repodata_gateway::Reporter;

pub mod cached_repo_data;
pub mod client;
pub mod middleware;

/// High-level function to fetch repodata for all the subdirectory of channels and platform.
/// Returns a list of `PyRepoData`.
#[pyfunction]
#[pyo3(signature = (channels, platforms, cache_path, callback=None, client=None))]
pub fn py_fetch_repo_data<'a>(
    py: Python<'a>,
    channels: Vec<PyChannel>,
    platforms: Vec<PyPlatform>,
    cache_path: PathBuf,
    callback: Option<Bound<'a, PyAny>>,
    client: Option<PyClientWithMiddleware>,
) -> PyResult<Bound<'a, PyAny>> {
    let mut meta_futures = Vec::new();
    let client = client.unwrap_or(PyClientWithMiddleware::new(None));

    for (subdir, chan) in get_subdir_urls(channels, platforms)? {
        let callback = callback.as_ref().map(|callback| {
            Arc::new(ProgressReporter {
                callback: callback.to_object(py),
            }) as _
        });
        let cache_path = cache_path.clone();
        let client = client.clone();

        // Push all the future into meta_future vec to be resolve later
        meta_futures.push(async move {
            Ok((
                fetch_repo_data(
                    subdir,
                    client.into(),
                    cache_path,
                    FetchRepoDataOptions::default(),
                    callback,
                )
                .await?,
                chan,
            )) as Result<(CachedRepoData, PyChannel), FetchRepoDataError>
        });
    }

    future_into_py(py, async move {
        // Resolve all the meta_futures together
        match try_join_all(meta_futures).await {
            Ok(res) => res
                .into_iter()
                .map(|(cache, chan)| {
                    let path = cache_path.to_string_lossy().into_owned();
                    PySparseRepoData::new(chan, path, cache.repo_data_json_path)
                })
                .collect::<Result<Vec<_>, _>>(),
            Err(e) => Err(PyRattlerError::from(e).into()),
        }
    })
}

struct ProgressReporter {
    callback: Py<PyAny>,
}

impl Reporter for ProgressReporter {
    fn on_download_progress(
        &self,
        _url: &Url,
        _index: usize,
        bytes_downloaded: usize,
        total_bytes: Option<usize>,
    ) {
        Python::with_gil(|py| {
            let args = PyTuple::new_bound(py, [Some(bytes_downloaded), total_bytes]);
            self.callback.call1(py, args).expect("Callback failed!");
        });
    }
}

/// Creates a subdir urls out of channels and channels.
fn get_subdir_urls(
    channels: Vec<PyChannel>,
    platforms: Vec<PyPlatform>,
) -> PyResult<Vec<(Url, PyChannel)>> {
    let mut urls = Vec::new();

    for c in channels {
        for p in &platforms {
            let r = c.platform_url(p);
            urls.push((
                Url::from_str(r.as_str()).map_err(PyRattlerError::from)?,
                c.clone(),
            ));
        }
    }

    Ok(urls)
}
