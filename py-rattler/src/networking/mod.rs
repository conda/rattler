use futures::{stream::FuturesUnordered, StreamExt};
use pyo3::{pyfunction, types::PyTuple, Py, PyAny, PyResult, Python, ToPyObject};
use pyo3_asyncio::tokio::future_into_py;
use rattler_repodata_gateway::fetch::{fetch_repo_data, DownloadProgress, FetchRepoDataOptions};
use url::Url;

use std::{path::Path, str::FromStr};

use crate::{channel::PyChannel, error::PyRattlerError, platform::PyPlatform};
use authenticated_client::PyAuthenticatedClient;
use cached_repo_data::PyCachedRepoData;

pub mod authenticated_client;
pub mod cached_repo_data;

#[pyfunction]
pub fn py_fetch_repo_data<'a>(
    py: Python<'a>,
    channels: Vec<PyChannel>,
    platforms: Vec<PyPlatform>,
    callback: Option<&'a PyAny>,
) -> PyResult<&'a PyAny> {
    let mut meta_futures = FuturesUnordered::new();

    let client = PyAuthenticatedClient::new();
    let cache_path = Path::new("/home/toaster/code/prefix/cache/");

    for subdir in get_subdir_urls(channels, platforms)? {
        let progress = if let Some(callback) = callback {
            let callback = callback.to_object(py);
            Some(get_closure(callback))
        } else {
            None
        };

        let client = client.clone();

        meta_futures.push(fetch_repo_data(
            subdir,
            client.into(),
            cache_path,
            FetchRepoDataOptions::default(),
            progress,
        ));
    }

    future_into_py(py, async move {
        let mut res: Vec<PyCachedRepoData> = Vec::new();

        while let Some(repo_data) = meta_futures.next().await {
            match repo_data {
                Ok(v) => res.push(v.into()),
                Err(e) => return Err(PyRattlerError::from(e).into()),
            }
        }

        Ok(res)
    })
}

fn get_closure(callback: Py<PyAny>) -> Box<dyn FnMut(DownloadProgress) + Send + Sync> {
    let cb = callback.clone();
    Box::new(move |progress: DownloadProgress| {
        Python::with_gil(|py| {
            let args = PyTuple::new(py, &[Some(progress.bytes), progress.total]);
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
