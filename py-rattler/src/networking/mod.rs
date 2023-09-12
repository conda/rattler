use futures::future::try_join_all;
use pyo3::{pyfunction, types::PyTuple, Py, PyAny, PyResult, Python, ToPyObject};
use pyo3_asyncio::tokio::future_into_py;

use rattler_repodata_gateway::fetch::{fetch_repo_data, DownloadProgress, FetchRepoDataOptions};
use url::Url;

use std::{path::PathBuf, str::FromStr};

use crate::{
    channel::PyChannel, error::PyRattlerError, platform::PyPlatform, repo_data::PyRepoData,
};
use authenticated_client::PyAuthenticatedClient;

pub mod authenticated_client;
pub mod cached_repo_data;

#[pyfunction]
pub fn py_fetch_repo_data<'a>(
    py: Python<'a>,
    channels: Vec<PyChannel>,
    platforms: Vec<PyPlatform>,
    cache_path: PathBuf,
    callback: Option<&'a PyAny>,
) -> PyResult<&'a PyAny> {
    let mut meta_futures = Vec::new();
    let client = PyAuthenticatedClient::new();

    for subdir in get_subdir_urls(channels, platforms)? {
        let progress = if let Some(callback) = callback {
            let callback = callback.to_object(py);
            Some(get_progress_func(callback))
        } else {
            None
        };
        let client = client.clone();

        // Push all the future into meta_future vec to be resolve later
        meta_futures.push(fetch_repo_data(
            subdir,
            client.into(),
            cache_path.clone(),
            FetchRepoDataOptions::default(),
            progress,
        ));
    }

    future_into_py(py, async move {
        // Resolve all the meta_futures together
        match try_join_all(meta_futures).await {
            Ok(cached_vec) => cached_vec
                .into_iter()
                .map(|c| PyRepoData::from_path(c.repo_data_json_path))
                .collect::<Result<Vec<_>, _>>(),
            Err(e) => Err(PyRattlerError::from(e).into()),
        }
    })
}

fn get_progress_func(callback: Py<PyAny>) -> Box<dyn FnMut(DownloadProgress) + Send + Sync> {
    let cb = callback;
    Box::new(move |progress: DownloadProgress| {
        Python::with_gil(|py| {
            let args = PyTuple::new(py, [Some(progress.bytes), progress.total]);
            cb.call1(py, args).expect("Callback failed!");
        });
    })
}

fn get_subdir_urls(channels: Vec<PyChannel>, platforms: Vec<PyPlatform>) -> PyResult<Vec<Url>> {
    let mut urls = Vec::new();

    for c in channels {
        for p in &platforms {
            let r = c.platform_url(p);
            urls.push(Url::from_str(r.as_str()).map_err(PyRattlerError::from)?);
        }
    }

    Ok(urls)
}
