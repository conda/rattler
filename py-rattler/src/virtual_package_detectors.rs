//! Bindings for channel-registered virtual package detectors.

use std::{collections::BTreeSet, path::PathBuf, sync::Arc, time::Duration};

use async_trait::async_trait;
use pyo3::{
    Bound, Py, PyAny, PyErr, PyResult, Python, pyclass, pyfunction, pymethods,
    types::{PyAnyMethods, PyBool, PyBoolMethods},
};
use pyo3_async_runtimes::tokio::future_into_py;
use rattler_networking::LazyClient;
use rattler_repodata_gateway::{
    AcceptedDetectorRegistration, RegistrationConflictKind, RejectedDetectorRegistration,
};
use rattler_virtual_package_detectors::{
    AllowAll, CacheClock, Consent, ConsentRequest, DenyAll, DetectOptions, DetectedValue,
    DetectionOutcome, DetectionSource, DetectorConsent, DetectorDiagnostics, DetectorFailure,
    DetectorResult, EnvironmentOptions, EnvironmentSnapshot, RattlerEnvironmentProvider,
    SkipReason, SkippedRegistration, WantedNames, detect, limits,
};

use crate::{
    PyChannel, error::PyRattlerError, generic_virtual_package::PyGenericVirtualPackage,
    networking::client::PyClientWithMiddleware, package_name::PyPackageName, record::PyRecord,
    repo_data::gateway::PyGateway, subdir::PySubdir,
};

/// A detector registration the gateway accepted.
#[pyclass(from_py_object)]
#[derive(Clone)]
pub struct PyDetectorRegistration {
    pub(crate) inner: AcceptedDetectorRegistration,
}

impl From<AcceptedDetectorRegistration> for PyDetectorRegistration {
    fn from(inner: AcceptedDetectorRegistration) -> Self {
        Self { inner }
    }
}

#[pymethods]
impl PyDetectorRegistration {
    #[getter]
    pub fn channel(&self) -> PyChannel {
        PyChannel {
            inner: self.inner.channel.clone(),
        }
    }

    #[getter]
    pub fn origin(&self) -> String {
        self.inner.origin().to_string()
    }

    #[getter]
    pub fn detector(&self) -> PyPackageName {
        self.inner.registration.detector.clone().into()
    }

    #[getter]
    pub fn virtual_packages(&self) -> Vec<PyPackageName> {
        self.inner
            .registration
            .virtual_packages
            .iter()
            .cloned()
            .map(|name| name.into_package_name().into())
            .collect()
    }

    #[getter]
    pub fn resolution_channels(&self) -> Vec<PyChannel> {
        self.inner
            .resolution_channels
            .iter()
            .cloned()
            .map(|inner| PyChannel { inner })
            .collect()
    }
}

/// A detector registration the gateway rejected for conflicting with an
/// accepted one.
#[pyclass(from_py_object)]
#[derive(Clone)]
pub struct PyRejectedDetectorRegistration {
    pub(crate) inner: RejectedDetectorRegistration,
}

impl From<RejectedDetectorRegistration> for PyRejectedDetectorRegistration {
    fn from(inner: RejectedDetectorRegistration) -> Self {
        Self { inner }
    }
}

#[pymethods]
impl PyRejectedDetectorRegistration {
    #[getter]
    pub fn channel(&self) -> PyChannel {
        PyChannel {
            inner: self.inner.channel.clone(),
        }
    }

    #[getter]
    pub fn detector(&self) -> PyPackageName {
        self.inner.registration.detector.clone().into()
    }

    #[getter]
    pub fn virtual_packages(&self) -> Vec<PyPackageName> {
        self.inner
            .registration
            .virtual_packages
            .iter()
            .cloned()
            .map(|name| name.into_package_name().into())
            .collect()
    }

    #[getter]
    pub fn conflicting_name(&self) -> PyPackageName {
        self.inner.conflict.name.clone().into()
    }

    /// `"name"` when the names are equal, otherwise the shared override
    /// variable.
    #[getter]
    pub fn conflict(&self) -> String {
        match &self.inner.conflict.kind {
            RegistrationConflictKind::Name => "name".to_string(),
            RegistrationConflictKind::OverrideVariable(variable) => variable.clone(),
        }
    }

    #[getter]
    pub fn accepted_origin(&self) -> String {
        self.inner.conflict.accepted_channel.to_string()
    }

    #[getter]
    pub fn accepted_detector(&self) -> PyPackageName {
        self.inner.conflict.accepted_detector.clone().into()
    }
}

/// What a consent policy is asked about.
#[pyclass(from_py_object)]
#[derive(Clone)]
pub struct PyConsentRequest {
    channel: PyChannel,
    detector: PyPackageName,
    virtual_packages: Vec<PyPackageName>,
    resolution_channels: Vec<PyChannel>,
    records: Vec<PyRecord>,
    digest: String,
}

impl PyConsentRequest {
    fn from_request(request: &ConsentRequest<'_>) -> Self {
        Self {
            channel: PyChannel {
                inner: request.channel.clone(),
            },
            detector: request.registration.detector.clone().into(),
            virtual_packages: request
                .registration
                .virtual_packages
                .iter()
                .cloned()
                .map(|name| name.into_package_name().into())
                .collect(),
            resolution_channels: request
                .resolution_channels
                .iter()
                .cloned()
                .map(|inner| PyChannel { inner })
                .collect(),
            records: request.records.iter().cloned().map(Into::into).collect(),
            digest: hex::encode(request.digest),
        }
    }
}

#[pymethods]
impl PyConsentRequest {
    #[getter]
    pub fn channel(&self) -> PyChannel {
        self.channel.clone()
    }

    #[getter]
    pub fn detector(&self) -> PyPackageName {
        self.detector.clone()
    }

    #[getter]
    pub fn virtual_packages(&self) -> Vec<PyPackageName> {
        self.virtual_packages.clone()
    }

    #[getter]
    pub fn resolution_channels(&self) -> Vec<PyChannel> {
        self.resolution_channels.clone()
    }

    #[getter]
    pub fn records(&self) -> Vec<PyRecord> {
        self.records.clone()
    }

    #[getter]
    pub fn digest(&self) -> String {
        self.digest.clone()
    }
}

/// A consent policy backed by a Python callable that receives a
/// [`PyConsentRequest`] and returns a truthy value to allow the detector.
///
/// An exception the callable raises denies the detector and is kept, so the
/// detection can re-raise it once it is done.
struct CallableConsent {
    callable: Py<PyAny>,
    error: std::sync::Mutex<Option<PyErr>>,
}

#[async_trait]
impl DetectorConsent for CallableConsent {
    async fn decide(&self, request: &ConsentRequest<'_>) -> Consent {
        let request = PyConsentRequest::from_request(request);
        let allowed = Python::attach(|py| -> PyResult<bool> {
            self.callable.bind(py).call1((request,))?.is_truthy()
        });
        match allowed {
            Ok(true) => Consent::Allow,
            Ok(false) => Consent::Deny,
            Err(err) => {
                let mut error = self
                    .error
                    .lock()
                    .expect("the consent error lock is never poisoned");
                error.get_or_insert(err);
                Consent::Deny
            }
        }
    }
}

enum ConsentPolicy {
    Allow(AllowAll),
    Deny(DenyAll),
    Callable(CallableConsent),
}

impl ConsentPolicy {
    fn from_py(consent: &Bound<'_, PyAny>) -> PyResult<Self> {
        if let Ok(flag) = consent.cast::<PyBool>() {
            return Ok(if flag.is_true() {
                Self::Allow(AllowAll)
            } else {
                Self::Deny(DenyAll)
            });
        }
        if consent.is_callable() {
            return Ok(Self::Callable(CallableConsent {
                callable: consent.clone().unbind(),
                error: std::sync::Mutex::new(None),
            }));
        }
        Err(pyo3::exceptions::PyTypeError::new_err(
            "consent must be a bool or a callable taking a ConsentRequest",
        ))
    }

    fn as_dyn(&self) -> &dyn DetectorConsent {
        match self {
            Self::Allow(policy) => policy,
            Self::Deny(policy) => policy,
            Self::Callable(policy) => policy,
        }
    }

    /// The exception the callback raised, if any.
    fn take_error(&self) -> Option<PyErr> {
        match self {
            Self::Callable(policy) => policy
                .error
                .lock()
                .expect("the consent error lock is never poisoned")
                .take(),
            Self::Allow(_) | Self::Deny(_) => None,
        }
    }
}

/// One virtual package name a detection decided about.
#[pyclass(from_py_object)]
#[derive(Clone)]
pub struct PyDetectorResult {
    inner: DetectorResult,
}

#[pymethods]
impl PyDetectorResult {
    #[getter]
    pub fn name(&self) -> PyPackageName {
        self.inner.name.clone().into()
    }

    /// The virtual package, or `None` when the name is absent.
    #[getter]
    pub fn virtual_package(&self) -> Option<PyGenericVirtualPackage> {
        self.inner.virtual_package().map(Into::into)
    }

    /// `"detector"` or `"override"`.
    #[getter]
    pub fn source(&self) -> &'static str {
        match self.inner.source {
            DetectionSource::Detector { .. } => "detector",
            DetectionSource::Override { .. } => "override",
        }
    }

    #[getter]
    pub fn origin(&self) -> Option<String> {
        match &self.inner.source {
            DetectionSource::Detector { origin, .. } => Some(origin.to_string()),
            DetectionSource::Override { .. } => None,
        }
    }

    #[getter]
    pub fn detector(&self) -> Option<PyPackageName> {
        match &self.inner.source {
            DetectionSource::Detector { detector, .. } => Some(detector.clone().into()),
            DetectionSource::Override { .. } => None,
        }
    }

    #[getter]
    pub fn digest(&self) -> Option<String> {
        match &self.inner.source {
            DetectionSource::Detector { digest, .. } => Some(hex::encode(digest)),
            DetectionSource::Override { .. } => None,
        }
    }

    #[getter(from_cache)]
    pub fn is_from_cache(&self) -> bool {
        matches!(
            self.inner.source,
            DetectionSource::Detector {
                from_cache: true,
                ..
            }
        )
    }

    #[getter]
    pub fn override_variable(&self) -> Option<String> {
        match &self.inner.source {
            DetectionSource::Override { variable } => Some(variable.clone()),
            DetectionSource::Detector { .. } => None,
        }
    }

    #[getter]
    pub fn absent(&self) -> bool {
        matches!(self.inner.value, DetectedValue::Absent)
    }
}

/// Diagnostics from one successful detector invocation.
#[pyclass(from_py_object)]
#[derive(Clone)]
pub struct PyDetectorDiagnostics {
    inner: Arc<DetectorDiagnostics>,
}

impl From<DetectorDiagnostics> for PyDetectorDiagnostics {
    fn from(diagnostics: DetectorDiagnostics) -> Self {
        Self {
            inner: Arc::new(diagnostics),
        }
    }
}

#[pymethods]
impl PyDetectorDiagnostics {
    #[getter]
    pub fn origin(&self) -> String {
        self.inner.origin.to_string()
    }

    #[getter]
    pub fn detector(&self) -> PyPackageName {
        self.inner.detector.clone().into()
    }

    #[getter]
    pub fn digest(&self) -> String {
        hex::encode(self.inner.digest)
    }

    #[getter(from_cache)]
    pub fn is_from_cache(&self) -> bool {
        self.inner.from_cache
    }

    #[getter]
    pub fn stderr(&self) -> &str {
        &self.inner.stderr
    }
}

/// A detector that failed; all of its results were discarded.
#[pyclass(from_py_object)]
#[derive(Clone)]
pub struct PyDetectorFailure {
    origin: String,
    detector: PyPackageName,
    message: String,
    stderr: Option<String>,
}

impl From<DetectorFailure> for PyDetectorFailure {
    fn from(failure: DetectorFailure) -> Self {
        let mut message = failure.error.to_string();
        let mut source = std::error::Error::source(&failure.error);
        while let Some(cause) = source {
            message.push_str(": ");
            message.push_str(&cause.to_string());
            source = cause.source();
        }
        Self {
            origin: failure.origin.to_string(),
            detector: failure.detector.into(),
            message,
            stderr: failure.stderr,
        }
    }
}

#[pymethods]
impl PyDetectorFailure {
    #[getter]
    pub fn origin(&self) -> String {
        self.origin.clone()
    }

    #[getter]
    pub fn detector(&self) -> PyPackageName {
        self.detector.clone()
    }

    #[getter]
    pub fn message(&self) -> String {
        self.message.clone()
    }

    #[getter]
    pub fn stderr(&self) -> Option<String> {
        self.stderr.clone()
    }
}

/// A detector that did not run.
#[pyclass(from_py_object)]
#[derive(Clone)]
pub struct PySkippedRegistration {
    origin: String,
    detector: PyPackageName,
    reason: &'static str,
    override_variables: Vec<String>,
}

impl From<SkippedRegistration> for PySkippedRegistration {
    fn from(skipped: SkippedRegistration) -> Self {
        let (reason, override_variables) = match skipped.reason {
            SkipReason::TargetIsNotHost { override_variables } => {
                ("target-is-not-host", override_variables)
            }
            SkipReason::NoWantedName => ("no-wanted-name", Vec::new()),
            SkipReason::ConsentDenied => ("consent-denied", Vec::new()),
        };
        Self {
            origin: skipped.origin.to_string(),
            detector: skipped.detector.into(),
            reason,
            override_variables,
        }
    }
}

#[pymethods]
impl PySkippedRegistration {
    #[getter]
    pub fn origin(&self) -> String {
        self.origin.clone()
    }

    #[getter]
    pub fn detector(&self) -> PyPackageName {
        self.detector.clone()
    }

    /// `"target-is-not-host"`, `"no-wanted-name"` or `"consent-denied"`.
    #[getter]
    pub fn reason(&self) -> &'static str {
        self.reason
    }

    #[getter]
    pub fn override_variables(&self) -> Vec<String> {
        self.override_variables.clone()
    }
}

/// Everything a detection produced.
#[pyclass(from_py_object)]
#[derive(Clone)]
pub struct PyDetectionOutcome {
    results: Vec<PyDetectorResult>,
    diagnostics: Vec<PyDetectorDiagnostics>,
    failures: Vec<PyDetectorFailure>,
    skipped: Vec<PySkippedRegistration>,
}

impl From<DetectionOutcome> for PyDetectionOutcome {
    fn from(outcome: DetectionOutcome) -> Self {
        Self {
            results: outcome
                .results
                .into_iter()
                .map(|inner| PyDetectorResult { inner })
                .collect(),
            diagnostics: outcome.diagnostics.into_iter().map(Into::into).collect(),
            failures: outcome.failures.into_iter().map(Into::into).collect(),
            skipped: outcome.skipped.into_iter().map(Into::into).collect(),
        }
    }
}

#[pymethods]
impl PyDetectionOutcome {
    #[getter]
    pub fn results(&self) -> Vec<PyDetectorResult> {
        self.results.clone()
    }

    #[getter]
    pub fn diagnostics(&self) -> Vec<PyDetectorDiagnostics> {
        self.diagnostics.clone()
    }

    #[getter]
    pub fn failures(&self) -> Vec<PyDetectorFailure> {
        self.failures.clone()
    }

    #[getter]
    pub fn skipped(&self) -> Vec<PySkippedRegistration> {
        self.skipped.clone()
    }

    /// Applies the results to `virtual_packages` the way a solve should see
    /// them: present results replace records of the same name, absent
    /// results remove them.
    pub fn merge(
        &self,
        virtual_packages: Vec<PyGenericVirtualPackage>,
    ) -> Vec<PyGenericVirtualPackage> {
        let results: Vec<DetectorResult> = self.results.iter().map(|r| r.inner.clone()).collect();
        rattler_virtual_package_detectors::merge_results(
            virtual_packages.into_iter().map(Into::into),
            &results,
        )
        .into_iter()
        .map(Into::into)
        .collect()
    }
}

/// Runs the given detectors and returns what they reported.
#[pyfunction]
#[pyo3(signature = (
    registrations,
    gateway,
    host_platform,
    target_platform,
    client_virtual_packages,
    consent,
    cache_dir=None,
    client=None,
    timeout_seconds=None,
    wanted=None,
    concurrency=4,
))]
#[allow(clippy::too_many_arguments)]
pub fn py_detect_virtual_packages<'py>(
    py: Python<'py>,
    registrations: Vec<PyDetectorRegistration>,
    gateway: PyGateway,
    host_platform: PySubdir,
    target_platform: PySubdir,
    client_virtual_packages: Vec<PyGenericVirtualPackage>,
    consent: Bound<'py, PyAny>,
    cache_dir: Option<PathBuf>,
    client: Option<PyClientWithMiddleware>,
    timeout_seconds: Option<u64>,
    wanted: Option<Vec<PyPackageName>>,
    concurrency: usize,
) -> PyResult<Bound<'py, PyAny>> {
    let consent = ConsentPolicy::from_py(&consent)?;
    let cache_dir = match cache_dir {
        Some(cache_dir) => cache_dir,
        None => rattler_cache::default_cache_dir().map_err(PyRattlerError::from)?,
    };
    let package_cache = gateway.inner.package_cache().clone();
    let download_client: LazyClient = match client {
        Some(client) => client.into(),
        None => LazyClient::from(PyClientWithMiddleware::new(None, None, None, None)?),
    };
    let registrations: Vec<AcceptedDetectorRegistration> =
        registrations.into_iter().map(|r| r.inner).collect();
    let wanted = match wanted {
        None => WantedNames::All,
        Some(names) => {
            WantedNames::Only(names.into_iter().map(Into::into).collect::<BTreeSet<_>>())
        }
    };
    let timeout = timeout_seconds.map_or(limits::DEFAULT_TIMEOUT, Duration::from_secs);
    let environment = EnvironmentSnapshot::from_system();
    let client_virtual_packages = client_virtual_packages
        .into_iter()
        .map(Into::into)
        .collect();

    future_into_py(py, async move {
        let root = cache_dir.join("virtual-package-detectors");
        let environment_root = root.join("envs");
        let environment_provider = RattlerEnvironmentProvider::new(EnvironmentOptions {
            gateway: &gateway.inner,
            package_cache: &package_cache,
            download_client,
            root: &environment_root,
            host_platform: host_platform.inner,
            virtual_packages: client_virtual_packages,
        });
        let outcome = detect(
            &registrations,
            DetectOptions {
                environment_provider: &environment_provider,
                environment: &environment,
                root: &root,
                host_platform: host_platform.inner,
                target_platform: target_platform.inner,
                timeout,
                consent: consent.as_dyn(),
                wanted,
                concurrency,
                clock: CacheClock::current(),
            },
        )
        .await
        .map_err(PyRattlerError::from)?;
        if let Some(error) = consent.take_error() {
            return Err(error);
        }
        Ok(PyDetectionOutcome::from(outcome))
    })
}
