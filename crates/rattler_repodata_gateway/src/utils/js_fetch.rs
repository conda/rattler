//! A fetch implementation provided by the host JavaScript environment.
//!
//! On wasm targets all HTTP requests normally go through the global `fetch`
//! function, which offers hosts no way to intercept the traffic of a single
//! [`crate::Gateway`]. A [`JsFetcher`] holds a `fetch`-like JavaScript
//! function that a gateway uses for all its requests instead, so hosts can
//! add authentication, route requests through their own HTTP stack, or
//! intercept them in tests.

use std::fmt;

use bytes::Bytes;
use js_sys::{Promise, Uint8Array};
use rattler_redaction::Redact;
use reqwest::StatusCode;
use send_wrapper::SendWrapper;
use url::Url;
use wasm_bindgen::{JsCast, JsValue};
use wasm_bindgen_futures::JsFuture;
use web_sys::{Request, RequestInit, Response};

/// A `fetch` implementation provided by the host JavaScript environment.
///
/// The wrapped function receives a `Request` object and must return a
/// promise resolving to a `Response`, mirroring the WHATWG `fetch`
/// function.
#[derive(Clone)]
pub struct JsFetcher {
    function: SendWrapper<js_sys::Function>,
}

impl fmt::Debug for JsFetcher {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("JsFetcher").finish_non_exhaustive()
    }
}

/// The successful result of a [`JsFetcher`] request.
pub struct JsFetchResponse {
    /// The status code of the response.
    pub status: StatusCode,

    /// The body of the response.
    pub bytes: Bytes,
}

/// An error returned by a [`JsFetcher`] request.
#[derive(Debug, thiserror::Error)]
pub enum JsFetchError {
    /// The server responded with a non-success status code.
    #[error("http status {status} for {url}")]
    Status {
        /// The status code of the response.
        status: StatusCode,
        /// The requested URL.
        url: Url,
    },

    /// The fetch function rejected or returned an unusable value.
    #[error("fetch failed for {url}: {message}")]
    Fetch {
        /// The requested URL.
        url: Url,
        /// A description of the failure.
        message: String,
    },
}

impl JsFetchError {
    /// Returns the status code of the response if the error was caused by a
    /// non-success status.
    pub fn status(&self) -> Option<StatusCode> {
        match self {
            JsFetchError::Status { status, .. } => Some(*status),
            JsFetchError::Fetch { .. } => None,
        }
    }
}

/// Extracts a human readable message from a JavaScript error value.
fn error_message(value: &JsValue) -> String {
    if let Some(error) = value.dyn_ref::<js_sys::Error>() {
        return String::from(error.message());
    }
    value.as_string().unwrap_or_else(|| format!("{value:?}"))
}

impl JsFetcher {
    /// Constructs a new instance from a `fetch`-like JavaScript function.
    pub fn new(function: js_sys::Function) -> Self {
        Self {
            function: SendWrapper::new(function),
        }
    }

    /// Executes a GET request for the given URL.
    pub async fn get(&self, url: &Url) -> Result<JsFetchResponse, JsFetchError> {
        let fetch_error = |value: &JsValue| JsFetchError::Fetch {
            url: url.clone().redact(),
            message: error_message(value),
        };
        let invalid = |message: &str| JsFetchError::Fetch {
            url: url.clone().redact(),
            message: message.to_string(),
        };

        let init = RequestInit::new();
        init.set_method("GET");
        let request =
            Request::new_with_str_and_init(url.as_str(), &init).map_err(|err| fetch_error(&err))?;

        let promise: Promise = self
            .function
            .call1(&JsValue::NULL, &request)
            .map_err(|err| fetch_error(&err))?
            .dyn_into()
            .map_err(|_| invalid("the fetch function did not return a promise"))?;
        let response: Response = JsFuture::from(promise)
            .await
            .map_err(|err| fetch_error(&err))?
            .dyn_into()
            .map_err(|_| invalid("the fetch function did not resolve to a response"))?;

        let status = StatusCode::from_u16(response.status())
            .map_err(|_| invalid("the response has an invalid status code"))?;
        if !status.is_success() {
            return Err(JsFetchError::Status {
                status,
                url: url.clone().redact(),
            });
        }

        let buffer = JsFuture::from(response.array_buffer().map_err(|err| fetch_error(&err))?)
            .await
            .map_err(|err| fetch_error(&err))?;
        let bytes = Bytes::from(Uint8Array::new(&buffer).to_vec());

        Ok(JsFetchResponse { status, bytes })
    }
}
