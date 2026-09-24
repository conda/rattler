use std::{path::PathBuf, sync::Arc};

use pyo3::{
    Bound, PyAny, PyResult, Python, exceptions::PyValueError, pyclass, pyfunction, pymethods,
};
use pyo3_async_runtimes::tokio::future_into_py;
use rattler_conda_types::RepoDataRecord;
use rattler_sigstore::{
    ChannelCheck, FulcioCiClaims, Issuer, Publisher, TrustedRoot, VerificationConfig,
    VerificationOutcome, VerificationPolicy, VerifiedAttestation, VerifiedChecks,
};

use crate::{error::PyRattlerError, networking::client::PyClientWithMiddleware, record::PyRecord};

/// A Sigstore trusted root, the trust anchor every bundle is verified against.
///
/// Wrapped in an `Arc` because pyo3 clones the class out of Python on every
/// call and a trusted root carries the full set of certificate authorities.
#[pyclass(from_py_object)]
#[derive(Clone)]
pub struct PyTrustedRoot {
    pub(crate) inner: Arc<TrustedRoot>,
}

#[pymethods]
impl PyTrustedRoot {
    /// Parses a trusted root from the JSON of a `trusted_root.json` target.
    #[staticmethod]
    pub fn from_json(json: &str) -> PyResult<Self> {
        TrustedRoot::from_json(json)
            .map(|root| Self {
                inner: Arc::new(root),
            })
            .map_err(|err| PyValueError::new_err(format!("invalid trusted root: {err}")))
    }

    /// Reads a trusted root from a `trusted_root.json` file.
    #[staticmethod]
    pub fn from_path(path: PathBuf) -> PyResult<Self> {
        TrustedRoot::from_file(&path)
            .map(|root| Self {
                inner: Arc::new(root),
            })
            .map_err(|err| {
                PyValueError::new_err(format!(
                    "could not read trusted root '{}': {err}",
                    path.display()
                ))
            })
    }
}

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

/// The claims a Fulcio signing certificate makes about the CI workload that
/// produced an attestation.
#[pyclass(from_py_object)]
#[derive(Clone)]
pub struct PyCertificateClaims {
    #[pyo3(get)]
    build_signer_uri: Option<String>,
    #[pyo3(get)]
    build_signer_digest: Option<String>,
    #[pyo3(get)]
    runner_environment: Option<String>,
    #[pyo3(get)]
    source_repository_uri: Option<String>,
    #[pyo3(get)]
    source_repository_digest: Option<String>,
    #[pyo3(get)]
    source_repository_ref: Option<String>,
    #[pyo3(get)]
    source_repository_identifier: Option<String>,
    #[pyo3(get)]
    source_repository_owner_uri: Option<String>,
    #[pyo3(get)]
    source_repository_owner_identifier: Option<String>,
    #[pyo3(get)]
    build_config_uri: Option<String>,
    #[pyo3(get)]
    build_config_digest: Option<String>,
    #[pyo3(get)]
    build_trigger: Option<String>,
    #[pyo3(get)]
    run_invocation_uri: Option<String>,
    #[pyo3(get)]
    source_repository_visibility_at_signing: Option<String>,
}

impl From<FulcioCiClaims> for PyCertificateClaims {
    fn from(value: FulcioCiClaims) -> Self {
        Self {
            build_signer_uri: value.build_signer_uri,
            build_signer_digest: value.build_signer_digest,
            runner_environment: value.runner_environment,
            source_repository_uri: value.source_repository_uri,
            source_repository_digest: value.source_repository_digest,
            source_repository_ref: value.source_repository_ref,
            source_repository_identifier: value.source_repository_identifier,
            source_repository_owner_uri: value.source_repository_owner_uri,
            source_repository_owner_identifier: value.source_repository_owner_identifier,
            build_config_uri: value.build_config_uri,
            build_config_digest: value.build_config_digest,
            build_trigger: value.build_trigger,
            run_invocation_uri: value.run_invocation_uri,
            source_repository_visibility_at_signing: value.source_repository_visibility_at_signing,
        }
    }
}

/// Which parts of the Sigstore verification of a bundle were performed.
#[pyclass(from_py_object)]
#[derive(Clone)]
pub struct PyVerifiedChecks {
    #[pyo3(get)]
    certificate_chain: bool,
    #[pyo3(get)]
    signed_certificate_timestamp: bool,
    #[pyo3(get)]
    transparency_log: bool,
    #[pyo3(get)]
    inclusion_proof: bool,
}

impl From<VerifiedChecks> for PyVerifiedChecks {
    fn from(value: VerifiedChecks) -> Self {
        Self {
            certificate_chain: value.certificate_chain,
            signed_certificate_timestamp: value.signed_certificate_timestamp,
            transparency_log: value.transparency_log,
            inclusion_proof: value.inclusion_proof,
        }
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
    claims: Option<PyCertificateClaims>,
    signed_at: Option<String>,
    log_index: Option<u64>,
    log_origin: Option<String>,
    checks: PyVerifiedChecks,
    warnings: Vec<String>,
}

impl From<VerifiedAttestation> for PyVerifiedAttestation {
    fn from(value: VerifiedAttestation) -> Self {
        Self {
            index: value.index,
            log_index: value.log_index(),
            log_origin: value.log_origin().map(str::to_owned),
            checks: value.checks.into(),
            identity: value.identity,
            issuer: value.issuer,
            integrated_time: value.integrated_time.map(|time| time.to_string()),
            target_channel: value.target_channel,
            // Fulcio certificates are short-lived, so the moment the
            // certificate became valid approximates the signing time.
            signed_at: value
                .certificate
                .as_ref()
                .map(|certificate| certificate.not_before.to_string()),
            claims: value
                .certificate
                .map(|certificate| certificate.ci_claims.into()),
            warnings: value.warnings,
        }
    }
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
    pub fn claims(&self) -> Option<PyCertificateClaims> {
        self.claims.clone()
    }

    #[getter]
    pub fn signed_at(&self) -> Option<&str> {
        self.signed_at.as_deref()
    }

    #[getter]
    pub fn log_index(&self) -> Option<u64> {
        self.log_index
    }

    #[getter]
    pub fn log_origin(&self) -> Option<&str> {
        self.log_origin.as_deref()
    }

    #[getter]
    pub fn checks(&self) -> PyVerifiedChecks {
        self.checks.clone()
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
            attestation: value.attestation.map(PyVerifiedAttestation::from),
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
#[pyo3(signature = (record, policy, client, trusted_root=None))]
pub fn py_verify_attestation<'py>(
    py: Python<'py>,
    record: Bound<'py, PyAny>,
    policy: PyVerificationPolicy,
    client: PyClientWithMiddleware,
    trusted_root: Option<PyTrustedRoot>,
) -> PyResult<Bound<'py, PyAny>> {
    let record: RepoDataRecord = PyRecord::try_from(record)?.try_into()?;
    future_into_py(py, async move {
        // Without an explicit root the production one is loaded over TUF, which
        // needs the network; with one, verification is entirely local apart
        // from fetching the sidecar.
        let outcome = match trusted_root {
            Some(trusted_root) => {
                rattler_sigstore::verify_record_with_trusted_root(
                    &policy.inner,
                    &record,
                    &client.inner,
                    &trusted_root.inner,
                )
                .await
            }
            None => rattler_sigstore::verify_record(&policy.inner, &record, &client.inner).await,
        };
        outcome
            .map(PyVerificationOutcome::from)
            .map_err(PyRattlerError::from)
            .map_err(Into::into)
    })
}
