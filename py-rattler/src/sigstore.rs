use pyo3::{
    Bound, PyAny, PyResult, Python, exceptions::PyValueError, pyclass, pyfunction, pymethods,
};
use pyo3_async_runtimes::tokio::future_into_py;
use rattler_conda_types::RepoDataRecord;
use rattler_sigstore::{
    ChannelCheck, Issuer, Publisher, VerificationConfig, VerificationOutcome, VerificationPolicy,
};

use crate::{error::PyRattlerError, networking::client::PyClientWithMiddleware, record::PyRecord};

#[pyclass(from_py_object)]
#[derive(Clone)]
pub struct PyVerificationPolicy {
    pub(crate) inner: VerificationPolicy,
}

fn parse_channel_check(value: &str) -> PyResult<ChannelCheck> {
    match value {
        "require" => Ok(ChannelCheck::Require),
        "warn" => Ok(ChannelCheck::Warn),
        "ignore" => Ok(ChannelCheck::Ignore),
        _ => Err(PyValueError::new_err(
            "channel_check must be 'require', 'warn', or 'ignore'",
        )),
    }
}

fn publisher(identity: Option<String>, issuer: Option<String>) -> PyResult<Publisher> {
    let publisher = identity.map_or_else(Publisher::new, |identity| {
        Publisher::new().with_identity(identity)
    });
    let Some(issuer) = issuer else {
        return Ok(publisher);
    };

    let url = url::Url::parse(&issuer)
        .map_err(|_err| PyValueError::new_err("issuer must be an absolute HTTP(S) URL"))?;
    if !matches!(url.scheme(), "http" | "https") || url.cannot_be_a_base() {
        return Err(PyValueError::new_err(
            "issuer must be an absolute HTTP(S) URL",
        ));
    }
    Ok(publisher.with_issuer(Issuer::new(issuer)))
}

fn verification_config(
    identity: Option<String>,
    issuer: Option<String>,
    channel_check: &str,
    max_sidecar_size: u64,
) -> PyResult<VerificationConfig> {
    Ok(VerificationConfig::new(publisher(identity, issuer)?)
        .with_channel_check(parse_channel_check(channel_check)?)
        .with_max_sidecar_size(max_sidecar_size))
}

#[pymethods]
impl PyVerificationPolicy {
    #[staticmethod]
    pub fn disabled() -> Self {
        Self {
            inner: VerificationPolicy::Disabled,
        }
    }

    #[staticmethod]
    #[pyo3(signature = (identity=None, issuer=None, channel_check="require", max_sidecar_size=rattler_sigstore::DEFAULT_MAX_SIDECAR_SIZE))]
    pub fn warn(
        identity: Option<String>,
        issuer: Option<String>,
        channel_check: &str,
        max_sidecar_size: u64,
    ) -> PyResult<Self> {
        Ok(Self {
            inner: VerificationPolicy::Warn(verification_config(
                identity,
                issuer,
                channel_check,
                max_sidecar_size,
            )?),
        })
    }

    #[staticmethod]
    #[pyo3(signature = (identity=None, issuer=None, channel_check="require", max_sidecar_size=rattler_sigstore::DEFAULT_MAX_SIDECAR_SIZE))]
    pub fn require(
        identity: Option<String>,
        issuer: Option<String>,
        channel_check: &str,
        max_sidecar_size: u64,
    ) -> PyResult<Self> {
        Ok(Self {
            inner: VerificationPolicy::Require(verification_config(
                identity,
                issuer,
                channel_check,
                max_sidecar_size,
            )?),
        })
    }

    #[getter]
    pub fn is_enabled(&self) -> bool {
        self.inner.is_enabled()
    }

    #[getter]
    pub fn is_required(&self) -> bool {
        self.inner.is_required()
    }
}

#[pyclass(from_py_object)]
#[derive(Clone)]
pub struct PyVerifiedAttestation {
    index: usize,
    identity: Option<String>,
    issuer: Option<String>,
    integrated_time: Option<String>,
    target_channel: Option<String>,
    warnings: Vec<String>,
}

#[pymethods]
impl PyVerifiedAttestation {
    #[getter]
    pub fn index(&self) -> usize {
        self.index
    }

    #[getter]
    pub fn identity(&self) -> Option<&str> {
        self.identity.as_deref()
    }

    #[getter]
    pub fn issuer(&self) -> Option<&str> {
        self.issuer.as_deref()
    }

    #[getter]
    pub fn integrated_time(&self) -> Option<&str> {
        self.integrated_time.as_deref()
    }

    #[getter]
    pub fn target_channel(&self) -> Option<&str> {
        self.target_channel.as_deref()
    }

    #[getter]
    pub fn warnings(&self) -> Vec<String> {
        self.warnings.clone()
    }
}

#[pyclass(from_py_object)]
#[derive(Clone)]
pub struct PyVerificationOutcome {
    attestation: Option<PyVerifiedAttestation>,
    warnings: Vec<String>,
}

impl From<VerificationOutcome> for PyVerificationOutcome {
    fn from(value: VerificationOutcome) -> Self {
        Self {
            attestation: value.attestation.map(|attestation| PyVerifiedAttestation {
                index: attestation.index,
                identity: attestation.identity,
                issuer: attestation.issuer,
                integrated_time: attestation
                    .integrated_time
                    .map(|timestamp| timestamp.to_string()),
                target_channel: attestation.target_channel,
                warnings: attestation.warnings,
            }),
            warnings: value.warnings,
        }
    }
}

#[pymethods]
impl PyVerificationOutcome {
    #[getter]
    pub fn attestation(&self) -> Option<PyVerifiedAttestation> {
        self.attestation.clone()
    }

    #[getter]
    pub fn warnings(&self) -> Vec<String> {
        self.warnings.clone()
    }

    #[getter]
    pub fn is_verified(&self) -> bool {
        self.attestation.is_some()
    }
}

#[pyfunction]
pub fn py_verify_attestation<'py>(
    py: Python<'py>,
    record: Bound<'py, PyAny>,
    policy: PyVerificationPolicy,
    client: PyClientWithMiddleware,
) -> PyResult<Bound<'py, PyAny>> {
    let record: RepoDataRecord = PyRecord::try_from(record)?.try_into()?;
    future_into_py(py, async move {
        rattler_sigstore::verify_record(&policy.inner, &record, &client.inner)
            .await
            .map(PyVerificationOutcome::from)
            .map_err(PyRattlerError::from)
            .map_err(Into::into)
    })
}
