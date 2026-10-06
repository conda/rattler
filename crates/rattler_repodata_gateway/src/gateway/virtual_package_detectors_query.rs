//! Collects the virtual package detectors that channels register in their
//! repodata, following the channel registration CEP.
//!
//! The query expands the given channels through their CEP 42 relations, reads
//! the `info.virtual_package_detectors` dictionaries of the explicitly supplied
//! subdirs, combines them per channel, and walks the channels in
//! resolved order: a registration is accepted unless one of its names, or one
//! of their override variables, already belongs to an accepted registration.
//! Every accepted registration also carries the channels its detector package
//! resolves against, which are the channels CEP 42 resolves for the registering
//! channel alone.

use std::{collections::HashMap, future::IntoFuture, sync::Arc};

use itertools::Itertools;
use rattler_conda_types::{
    Channel, ChannelUrl, PackageName, Subdir,
    virtual_package_detector::{
        ChannelDetectorRegistrations, DetectorRegistration, InvalidVirtualPackageNameError,
        RegistrationError, SubdirDetectorRegistrations,
    },
};

use super::{
    GatewayError, GatewayInner, GatewayWarning,
    boxed::{BoxFuture, box_future},
    channel_expander::ChannelRelationsMode,
    channel_expansion::{ChannelExpansion, expand_channels},
    channel_relations::DEFAULT_CHANNEL_RELATIONS_MAX_DEPTH,
};
use crate::Reporter;

/// A registration the query accepted: its detector speaks for its virtual
/// package names in the solve.
#[derive(Debug, Clone)]
pub struct AcceptedDetectorRegistration {
    /// The channel that registered the detector. Its base URL is the
    /// registration's origin.
    pub channel: Channel,
    /// The detector and the virtual packages it reports.
    pub registration: DetectorRegistration,
    /// The channels the detector package and its dependencies resolve
    /// against, highest priority first. The registering channel is always
    /// among them.
    pub resolution_channels: Vec<Channel>,
}

impl AcceptedDetectorRegistration {
    /// The registration's origin: the registering channel's base URL.
    pub fn origin(&self) -> &ChannelUrl {
        &self.channel.base_url
    }
}

/// A registration the query rejected because a lower-priority channel
/// registered a name an accepted registration already reserved. Its detector
/// must not run.
#[derive(Debug, Clone)]
pub struct RejectedDetectorRegistration {
    /// The channel that registered the detector.
    pub channel: Channel,
    /// The detector and the virtual packages it would have reported.
    pub registration: DetectorRegistration,
    /// The first conflicting name and what it conflicts with.
    pub conflict: RegistrationConflict,
}

/// How a rejected registration conflicts with an accepted one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RegistrationConflict {
    /// The rejected registration's conflicting virtual package name.
    pub name: PackageName,
    /// Whether the names are equal or only share an override variable.
    pub kind: RegistrationConflictKind,
    /// The channel of the accepted registration.
    pub accepted_channel: ChannelUrl,
    /// The detector of the accepted registration.
    pub accepted_detector: PackageName,
    /// The accepted registration's name that reserved the slot.
    pub accepted_name: PackageName,
}

/// The kind of a [`RegistrationConflict`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RegistrationConflictKind {
    /// Both registrations use the same normalized name.
    Name,
    /// The names differ but map to the same `CONDA_OVERRIDE_*` variable.
    OverrideVariable(String),
}

/// A non-fatal issue surfaced while collecting detector registrations.
#[derive(Debug, Clone, thiserror::Error)]
pub enum VirtualPackageDetectorWarning {
    /// A channel's combined registration set is invalid and was ignored.
    #[error("ignoring the virtual package detectors registered by {channel}: {error}")]
    InvalidRegistrations {
        /// The channel whose registrations were ignored.
        channel: ChannelUrl,
        /// Why the set is invalid.
        error: RegistrationError,
    },

    /// A registered virtual package name is invalid and was dropped.
    #[error(
        "ignoring virtual package name {name:?} registered by detector '{}' of {channel}: {reason}",
        detector.as_source()
    )]
    DroppedName {
        /// The channel that registered the name.
        channel: ChannelUrl,
        /// The detector that registered the name.
        detector: PackageName,
        /// The name as the channel wrote it.
        name: String,
        /// Why the name is invalid.
        reason: InvalidVirtualPackageNameError,
    },

    /// A registration lost a name to a higher-priority channel and was
    /// rejected as a whole.
    #[error(
        "not running detector '{}' of {channel}: virtual package '{}' is already provided by detector '{}' of {}",
        detector.as_source(),
        conflict.name.as_normalized(),
        conflict.accepted_detector.as_source(),
        conflict.accepted_channel
    )]
    RejectedRegistration {
        /// The channel that registered the rejected detector.
        channel: ChannelUrl,
        /// The rejected detector.
        detector: PackageName,
        /// What the registration conflicts with.
        conflict: RegistrationConflict,
    },

    /// A cycle or the depth limit prevented resolving a registering channel's
    /// relations, so its detectors resolve from the channel alone.
    #[error(
        "resolving the virtual package detectors of {channel} from the channel alone, its channel relations could not be followed: {reason}"
    )]
    ResolutionFallback {
        /// The registering channel.
        channel: ChannelUrl,
        /// The relation warnings that caused the fallback.
        reason: String,
    },
}

/// The result of a [`VirtualPackageDetectorsQuery`].
#[derive(Debug, Default)]
pub struct VirtualPackageDetectorsOutput {
    /// The accepted registrations in CEP 42 channel order. Every virtual
    /// package name appears in exactly one of them.
    pub registrations: Vec<AcceptedDetectorRegistration>,
    /// The registrations rejected for conflicting with an accepted one.
    pub rejected: Vec<RejectedDetectorRegistration>,
    /// Non-fatal issues: invalid registration sets, dropped names, rejected
    /// registrations, resolution fallbacks and channel relation warnings.
    /// Also streamed to [`Reporter::on_gateway_warning`] as they are recorded.
    pub warnings: Vec<GatewayWarning>,
}

/// A query for the virtual package detectors registered by a set of channels.
/// Create it with [`Gateway::virtual_package_detectors`](super::Gateway::virtual_package_detectors).
#[derive(Clone)]
pub struct VirtualPackageDetectorsQuery {
    gateway: Arc<GatewayInner>,
    channels: Vec<Channel>,
    platforms: Vec<Subdir>,
    reporter: Option<Arc<dyn Reporter>>,
    channel_relations_mode: ChannelRelationsMode,
    channel_relations_max_depth: usize,
}

impl VirtualPackageDetectorsQuery {
    pub(super) fn new(
        gateway: Arc<GatewayInner>,
        channels: Vec<Channel>,
        platforms: Vec<Subdir>,
        reporter: Option<Arc<dyn Reporter>>,
    ) -> Self {
        Self {
            gateway,
            channels,
            platforms: platforms.into_iter().unique().collect(),
            reporter,
            channel_relations_mode: ChannelRelationsMode::default(),
            channel_relations_max_depth: DEFAULT_CHANNEL_RELATIONS_MAX_DEPTH,
        }
    }

    /// Sets the reporter that is notified of warnings as they are recorded.
    pub fn with_reporter(self, reporter: impl Reporter + 'static) -> Self {
        Self {
            reporter: Some(Arc::new(reporter)),
            ..self
        }
    }

    /// How to treat CEP 42 `channel_relations`. Defaults to
    /// [`ChannelRelationsMode::Warn`].
    #[must_use]
    pub fn channel_relations(self, mode: ChannelRelationsMode) -> Self {
        Self {
            channel_relations_mode: mode,
            ..self
        }
    }

    /// Maximum CEP 42 recursion depth. Defaults to
    /// [`DEFAULT_CHANNEL_RELATIONS_MAX_DEPTH`]. No effect when the mode is
    /// [`ChannelRelationsMode::Disabled`].
    #[must_use]
    pub fn channel_relations_max_depth(self, depth: usize) -> Self {
        Self {
            channel_relations_max_depth: depth,
            ..self
        }
    }

    /// Executes the query.
    pub async fn execute(self) -> Result<VirtualPackageDetectorsOutput, GatewayError> {
        let platforms = &self.platforms;
        let expansion = expand_channels(
            &self.gateway,
            self.channels.clone(),
            platforms.clone(),
            self.channel_relations_mode,
            self.channel_relations_max_depth,
            self.reporter.clone(),
            None,
        )
        .await?;
        self.collect_from_expansion(&expansion).await
    }

    pub(super) async fn collect_from_expansion(
        &self,
        expansion: &ChannelExpansion,
    ) -> Result<VirtualPackageDetectorsOutput, GatewayError> {
        let mut output = VirtualPackageDetectorsOutput::default();
        for warning in &expansion.warnings {
            output.warnings.push(GatewayWarning::from(warning.clone()));
        }

        let platforms = &self.platforms;
        let mut reserved = ReservedNames::default();
        let mut resolutions: HashMap<ChannelUrl, Vec<Channel>> = HashMap::new();
        for channel in expansion.ordered_channels() {
            let url = &channel.base_url;
            let combined = match combine_subdirs(expansion, url, platforms) {
                Ok(combined) => combined,
                Err(error) => {
                    self.warn(
                        &mut output,
                        VirtualPackageDetectorWarning::InvalidRegistrations {
                            channel: url.clone(),
                            error,
                        },
                    );
                    continue;
                }
            };
            for dropped in combined.dropped_names() {
                self.warn(
                    &mut output,
                    VirtualPackageDetectorWarning::DroppedName {
                        channel: url.clone(),
                        detector: dropped.detector.clone(),
                        name: dropped.name.clone(),
                        reason: dropped.reason.clone(),
                    },
                );
            }

            for registration in combined.into_registrations() {
                if let Some(conflict) = reserved.conflict(&registration) {
                    self.warn(
                        &mut output,
                        VirtualPackageDetectorWarning::RejectedRegistration {
                            channel: url.clone(),
                            detector: registration.detector.clone(),
                            conflict: conflict.clone(),
                        },
                    );
                    output.rejected.push(RejectedDetectorRegistration {
                        channel: Channel::clone(channel),
                        registration,
                        conflict,
                    });
                    continue;
                }
                reserved.reserve(url, &registration);

                let resolution_channels = if let Some(channels) = resolutions.get(url) {
                    channels.clone()
                } else {
                    let channels = self
                        .resolution_channels(channel, platforms, expansion, &mut output)
                        .await?;
                    resolutions.insert(url.clone(), channels.clone());
                    channels
                };
                output.registrations.push(AcceptedDetectorRegistration {
                    channel: Channel::clone(channel),
                    registration,
                    resolution_channels,
                });
            }
        }

        Ok(output)
    }

    /// The channels CEP 42 resolves for `channel` alone. Falls back to the
    /// channel by itself, with a warning, when a cycle or the depth limit
    /// prevents following its relations.
    async fn resolution_channels(
        &self,
        channel: &Arc<Channel>,
        platforms: &[Subdir],
        previous: &ChannelExpansion,
        output: &mut VirtualPackageDetectorsOutput,
    ) -> Result<Vec<Channel>, GatewayError> {
        // The CEP asks for a fallback to the channel alone when its relations
        // cannot be followed, so this expansion never aborts, whatever mode
        // the query runs in.
        let mode = match self.channel_relations_mode {
            ChannelRelationsMode::Disabled => ChannelRelationsMode::Disabled,
            ChannelRelationsMode::Warn | ChannelRelationsMode::Strict => ChannelRelationsMode::Warn,
        };
        let expansion = expand_channels(
            &self.gateway,
            vec![Channel::clone(channel)],
            platforms.to_vec(),
            mode,
            self.channel_relations_max_depth,
            self.reporter.clone(),
            Some(previous),
        )
        .await?;
        if expansion.was_cut_short() {
            let reason = expansion
                .warnings
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join("; ");
            self.warn(
                output,
                VirtualPackageDetectorWarning::ResolutionFallback {
                    channel: channel.base_url.clone(),
                    reason,
                },
            );
            return Ok(vec![Channel::clone(channel)]);
        }
        Ok(expansion
            .ordered_channels()
            .map(|channel| Channel::clone(channel))
            .collect())
    }

    fn warn(
        &self,
        output: &mut VirtualPackageDetectorsOutput,
        warning: VirtualPackageDetectorWarning,
    ) {
        let warning = GatewayWarning::from(warning);
        if let Some(reporter) = &self.reporter {
            reporter.on_gateway_warning(&warning);
        }
        output.warnings.push(warning);
    }
}

/// Parses and combines the registrations of one channel's loaded subdirs.
fn combine_subdirs(
    expansion: &ChannelExpansion,
    url: &ChannelUrl,
    platforms: &[Subdir],
) -> Result<ChannelDetectorRegistrations, RegistrationError> {
    let subdirs = platforms
        .iter()
        .map(|&platform| {
            let raw = expansion
                .subdir(url, platform)
                .and_then(|subdir| subdir.virtual_package_detectors());
            SubdirDetectorRegistrations::parse(raw)
        })
        .collect::<Result<Vec<_>, _>>()?;
    ChannelDetectorRegistrations::combine(&subdirs)
}

/// The names and override variables accepted registrations have reserved.
#[derive(Default)]
struct ReservedNames {
    names: HashMap<String, Reservation>,
    variables: HashMap<String, Reservation>,
}

#[derive(Clone)]
struct Reservation {
    channel: ChannelUrl,
    detector: PackageName,
    name: PackageName,
}

impl ReservedNames {
    fn conflict(&self, registration: &DetectorRegistration) -> Option<RegistrationConflict> {
        registration.virtual_packages.iter().find_map(|name| {
            let (kind, reservation) =
                if let Some(reservation) = self.names.get(name.as_normalized()) {
                    (RegistrationConflictKind::Name, reservation)
                } else {
                    let variable = name.override_variable();
                    let reservation = self.variables.get(&variable)?;
                    (
                        RegistrationConflictKind::OverrideVariable(variable),
                        reservation,
                    )
                };
            Some(RegistrationConflict {
                name: name.as_package_name().clone(),
                kind,
                accepted_channel: reservation.channel.clone(),
                accepted_detector: reservation.detector.clone(),
                accepted_name: reservation.name.clone(),
            })
        })
    }

    fn reserve(&mut self, channel: &ChannelUrl, registration: &DetectorRegistration) {
        for name in &registration.virtual_packages {
            let reservation = Reservation {
                channel: channel.clone(),
                detector: registration.detector.clone(),
                name: name.as_package_name().clone(),
            };
            self.names
                .insert(name.as_normalized().to_string(), reservation.clone());
            self.variables.insert(name.override_variable(), reservation);
        }
    }
}

impl IntoFuture for VirtualPackageDetectorsQuery {
    type Output = Result<VirtualPackageDetectorsOutput, GatewayError>;
    type IntoFuture = BoxFuture<Self::Output>;

    fn into_future(self) -> Self::IntoFuture {
        box_future(self.execute())
    }
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests {
    use std::{collections::BTreeSet, path::Path};

    use rattler_conda_types::{Channel, ChannelUrl, Subdir, VirtualPackageName};

    use super::*;
    use crate::{Gateway, utils::simple_channel_server::SimpleChannelServer};

    /// Writes a subdir whose `info` carries the given raw
    /// `virtual_package_detectors` JSON and optional relations.
    fn write_subdir(
        root: &Path,
        subdir: Subdir,
        detectors: Option<&str>,
        base: Option<&str>,
        overrides: Option<&str>,
    ) {
        let mut info = vec![format!("\"subdir\": \"{subdir}\"")];
        if let Some(detectors) = detectors {
            info.push(format!("\"virtual_package_detectors\": {detectors}"));
        }
        let mut relations = Vec::new();
        if let Some(base) = base {
            relations.push(format!("\"base\": \"{base}\""));
        }
        if let Some(overrides) = overrides {
            relations.push(format!("\"overrides\": \"{overrides}\""));
        }
        if !relations.is_empty() {
            info.push(format!(
                "\"channel_relations\": {{{}}}",
                relations.join(", ")
            ));
        }
        let json = format!(
            r#"{{"info": {{{}}}, "packages": {{}}, "packages.conda": {{}}}}"#,
            info.join(", ")
        );
        let dir = root.join(subdir.as_str());
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("repodata.json"), json).unwrap();
    }

    fn add_candidate(
        root: &Path,
        subdir: Subdir,
        name: &str,
        version: &str,
        depends: &[&str],
        constrains: &[&str],
    ) {
        let path = root.join(subdir.as_str()).join("repodata.json");
        let mut repodata: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        repodata["packages"][format!("{name}-{version}-0.tar.bz2")] = serde_json::json!({
            "name": name, "version": version, "build": "0", "build_number": 0,
            "subdir": subdir.as_str(), "depends": depends, "constrains": constrains,
        });
        std::fs::write(path, serde_json::to_vec(&repodata).unwrap()).unwrap();
    }

    fn channel(server: &SimpleChannelServer, name: &str) -> Channel {
        Channel::from_url(server.url().join(&format!("{name}/")).unwrap())
    }

    fn url(server: &SimpleChannelServer, name: &str) -> ChannelUrl {
        channel(server, name).base_url
    }

    fn names(registration: &AcceptedDetectorRegistration) -> Vec<&str> {
        registration
            .registration
            .virtual_packages
            .iter()
            .map(VirtualPackageName::as_normalized)
            .collect()
    }

    fn resolution(registration: &AcceptedDetectorRegistration) -> Vec<ChannelUrl> {
        registration
            .resolution_channels
            .iter()
            .map(|channel| channel.base_url.clone())
            .collect()
    }

    #[tokio::test]
    async fn combines_subdir_and_noarch_in_channel_order() {
        let dir = tempfile::tempdir().unwrap();
        let forge = dir.path().join("conda-forge");
        let bio = dir.path().join("bioconda");
        write_subdir(
            &forge,
            Subdir::Linux64,
            Some(r#"{"mpi-detect": ["__conda_forge_openmpi", "__conda_forge_mpich"]}"#),
            None,
            None,
        );
        write_subdir(
            &forge,
            Subdir::NoArch,
            Some(
                r#"{"mpi-detect": ["__conda_forge_openmpi", "__conda_forge_mpich"], "cuda-detect": ["__cuda"]}"#,
            ),
            None,
            None,
        );
        write_subdir(
            &bio,
            Subdir::Linux64,
            Some(r#"{"bio-detect": ["__bioconda_blast"]}"#),
            Some("../conda-forge"),
            None,
        );

        let server = SimpleChannelServer::new(dir.path()).await;
        let output = Gateway::new()
            .virtual_package_detectors(
                [channel(&server, "bioconda")],
                [Subdir::Linux64, Subdir::NoArch],
            )
            .await
            .unwrap();

        assert!(output.rejected.is_empty());
        assert!(output.warnings.is_empty(), "{:?}", output.warnings);
        let summary: Vec<_> = output
            .registrations
            .iter()
            .map(|r| {
                (
                    r.origin().clone(),
                    r.registration.detector.as_source(),
                    names(r),
                )
            })
            .collect();
        assert_eq!(
            summary,
            [
                (
                    url(&server, "conda-forge"),
                    "mpi-detect",
                    vec!["__conda_forge_openmpi", "__conda_forge_mpich"]
                ),
                (url(&server, "conda-forge"), "cuda-detect", vec!["__cuda"]),
                (
                    url(&server, "bioconda"),
                    "bio-detect",
                    vec!["__bioconda_blast"]
                ),
            ]
        );
        assert_eq!(
            resolution(&output.registrations[0]),
            [url(&server, "conda-forge")]
        );
        assert_eq!(
            resolution(&output.registrations[2]),
            [url(&server, "conda-forge"), url(&server, "bioconda")]
        );
    }

    #[tokio::test]
    async fn higher_priority_channel_wins_conflicts() {
        let dir = tempfile::tempdir().unwrap();
        let forge = dir.path().join("conda-forge");
        let bio = dir.path().join("bioconda");
        write_subdir(
            &forge,
            Subdir::Linux64,
            Some(r#"{"cuda-detect": ["__cuda", "__conda-forge_mpi"]}"#),
            None,
            None,
        );
        write_subdir(
            &bio,
            Subdir::Linux64,
            Some(
                r#"{"bio-cuda": ["__bioconda_x", "__cuda"], "bio-mpi": ["__conda_forge_mpi"], "bio-ok": ["__bioconda_y"]}"#,
            ),
            Some("../conda-forge"),
            None,
        );

        let server = SimpleChannelServer::new(dir.path()).await;
        let output = Gateway::new()
            .virtual_package_detectors(
                [channel(&server, "bioconda")],
                [Subdir::Linux64, Subdir::NoArch],
            )
            .await
            .unwrap();

        let accepted: Vec<_> = output
            .registrations
            .iter()
            .map(|r| r.registration.detector.as_source())
            .collect();
        assert_eq!(accepted, ["cuda-detect", "bio-ok"]);

        let rejected: Vec<_> = output
            .rejected
            .iter()
            .map(|r| {
                (
                    r.registration.detector.as_source(),
                    r.conflict.name.as_normalized(),
                    r.conflict.kind.clone(),
                    r.conflict.accepted_detector.as_source(),
                )
            })
            .collect();
        assert_eq!(
            rejected,
            [
                (
                    "bio-cuda",
                    "__cuda",
                    RegistrationConflictKind::Name,
                    "cuda-detect"
                ),
                (
                    "bio-mpi",
                    "__conda_forge_mpi",
                    RegistrationConflictKind::OverrideVariable(
                        "CONDA_OVERRIDE_CONDA_FORGE_MPI".to_string()
                    ),
                    "cuda-detect"
                ),
            ]
        );
        assert_eq!(
            output
                .warnings
                .iter()
                .filter(|w| matches!(
                    w,
                    GatewayWarning::VirtualPackageDetectors(
                        VirtualPackageDetectorWarning::RejectedRegistration { .. }
                    )
                ))
                .count(),
            2
        );
    }

    #[tokio::test]
    async fn invalid_registrations_are_reported_and_skipped() {
        let dir = tempfile::tempdir().unwrap();
        let broken = dir.path().join("broken");
        let fine = dir.path().join("fine");
        write_subdir(
            &broken,
            Subdir::Linux64,
            Some(r#"{"-invalid": ["__a"]}"#),
            None,
            None,
        );
        write_subdir(
            &fine,
            Subdir::Linux64,
            Some(r#"{"a-detect": ["__a", "not-a-virtual-package"]}"#),
            None,
            None,
        );
        write_subdir(
            &fine,
            Subdir::NoArch,
            Some(r#"{"a-detect": ["__a", "not-a-virtual-package"]}"#),
            None,
            None,
        );

        let server = SimpleChannelServer::new(dir.path()).await;
        let output = Gateway::new()
            .virtual_package_detectors(
                [channel(&server, "broken"), channel(&server, "fine")],
                [Subdir::Linux64, Subdir::NoArch],
            )
            .await
            .unwrap();

        assert_eq!(output.registrations.len(), 1);
        assert_eq!(names(&output.registrations[0]), ["__a"]);
        assert!(output.warnings.iter().any(|warning| matches!(
            warning,
            GatewayWarning::VirtualPackageDetectors(
                VirtualPackageDetectorWarning::InvalidRegistrations { channel: origin, .. }
            ) if origin == &url(&server, "broken")
        )));
        assert!(output.warnings.iter().any(|warning| matches!(
            warning,
            GatewayWarning::VirtualPackageDetectors(
                VirtualPackageDetectorWarning::DroppedName { name, .. }
            ) if name == "not-a-virtual-package"
        )));
    }

    #[tokio::test]
    async fn supplied_subdirs_control_registration_discovery() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("channel");
        write_subdir(
            &root,
            Subdir::Linux64,
            Some(r#"{"platform-detector": ["__platform"]}"#),
            None,
            None,
        );
        write_subdir(
            &root,
            Subdir::NoArch,
            Some(r#"{"noarch-detector": ["__portable"]}"#),
            None,
            None,
        );
        let server = SimpleChannelServer::new(dir.path()).await;
        let gateway = Gateway::new();

        let platform_only = gateway
            .virtual_package_detectors([channel(&server, "channel")], [Subdir::Linux64])
            .await
            .unwrap();
        assert_eq!(platform_only.registrations.len(), 1);
        assert_eq!(names(&platform_only.registrations[0]), ["__platform"]);

        let noarch_only = gateway
            .virtual_package_detectors(
                [channel(&server, "channel")],
                [Subdir::NoArch, Subdir::NoArch],
            )
            .await
            .unwrap();
        assert_eq!(noarch_only.registrations.len(), 1);
        assert_eq!(names(&noarch_only.registrations[0]), ["__portable"]);
        assert!(noarch_only.rejected.is_empty());

        let combined = gateway
            .virtual_package_detectors(
                [channel(&server, "channel")],
                [Subdir::Linux64, Subdir::NoArch],
            )
            .await
            .unwrap();
        assert_eq!(
            combined
                .registrations
                .iter()
                .flat_map(names)
                .collect::<Vec<_>>(),
            ["__platform", "__portable"],
        );
    }

    #[tokio::test]
    async fn malformed_registration_metadata_is_a_repodata_error() {
        for metadata in [
            "null",
            "[]",
            r#"{"detector": "__name"}"#,
            r#"{"detector": [42]}"#,
        ] {
            let dir = tempfile::tempdir().unwrap();
            write_subdir(
                &dir.path().join("channel"),
                Subdir::Linux64,
                Some(metadata),
                None,
                None,
            );
            let server = SimpleChannelServer::new(dir.path()).await;
            assert!(
                Gateway::new()
                    .virtual_package_detectors([channel(&server, "channel")], [Subdir::Linux64])
                    .await
                    .is_err(),
                "malformed registration metadata was accepted: {metadata}",
            );
        }
    }

    #[tokio::test]
    async fn inconsistent_subdirs_discard_the_channel() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("channel");
        write_subdir(
            &root,
            Subdir::Linux64,
            Some(r#"{"a-detect": ["__a"]}"#),
            None,
            None,
        );
        write_subdir(
            &root,
            Subdir::NoArch,
            Some(r#"{"a-detect": ["__b"]}"#),
            None,
            None,
        );

        let server = SimpleChannelServer::new(dir.path()).await;
        let output = Gateway::new()
            .virtual_package_detectors(
                [channel(&server, "channel")],
                [Subdir::Linux64, Subdir::NoArch],
            )
            .await
            .unwrap();
        assert!(output.registrations.is_empty());
        assert!(matches!(
            output.warnings.as_slice(),
            [GatewayWarning::VirtualPackageDetectors(
                VirtualPackageDetectorWarning::InvalidRegistrations {
                    error: RegistrationError::InconsistentAcrossSubdirs { .. },
                    ..
                }
            )]
        ));
    }

    #[tokio::test]
    async fn invalid_detector_key_discards_channel_before_reserving_names() {
        for key in ["", "-bad", "bad..name", &"a".repeat(65)] {
            let dir = tempfile::tempdir().unwrap();
            let invalid = serde_json::json!({key: ["__shared"]}).to_string();
            write_subdir(
                &dir.path().join("broken"),
                Subdir::Linux64,
                Some(&invalid),
                None,
                None,
            );
            write_subdir(
                &dir.path().join("broken"),
                Subdir::NoArch,
                Some(r#"{"otherwise-valid": ["__other"]}"#),
                None,
                None,
            );
            write_subdir(
                &dir.path().join("fine"),
                Subdir::Linux64,
                Some(r#"{"valid": ["__shared", "__other"]}"#),
                None,
                None,
            );
            let server = SimpleChannelServer::new(dir.path()).await;
            let output = Gateway::new()
                .virtual_package_detectors(
                    [channel(&server, "broken"), channel(&server, "fine")],
                    [Subdir::Linux64, Subdir::NoArch],
                )
                .await
                .unwrap();
            assert_eq!(
                output
                    .registrations
                    .iter()
                    .map(|r| r.registration.detector.as_source())
                    .collect::<Vec<_>>(),
                ["valid"],
                "invalid detector key {key:?} reserved a name"
            );
            assert!(output.rejected.is_empty());
            assert!(output.warnings.iter().any(|warning| matches!(
                warning,
                GatewayWarning::VirtualPackageDetectors(
                    VirtualPackageDetectorWarning::InvalidRegistrations { channel, .. }
                ) if channel == &url(&server, "broken")
            )));
        }
    }

    #[tokio::test]
    async fn discovered_subdir_failure_preserves_independent_registrations() {
        let dir = tempfile::tempdir().unwrap();
        write_subdir(
            &dir.path().join("root"),
            Subdir::Linux64,
            None,
            Some("../discovered"),
            None,
        );
        write_subdir(
            &dir.path().join("discovered"),
            Subdir::NoArch,
            Some(r#"{"discovered-detector": ["__discovered"]}"#),
            None,
            None,
        );
        let failed = dir.path().join("discovered/linux-64");
        std::fs::create_dir_all(&failed).unwrap();
        std::fs::write(failed.join("repodata.json"), "{ malformed repodata").unwrap();
        write_subdir(
            &dir.path().join("independent"),
            Subdir::Linux64,
            Some(r#"{"independent-detector": ["__independent"]}"#),
            None,
            None,
        );
        let server = SimpleChannelServer::new(dir.path()).await;
        let channels = [channel(&server, "root"), channel(&server, "independent")];
        let output = Gateway::new()
            .virtual_package_detectors(channels.clone(), [Subdir::Linux64, Subdir::NoArch])
            .channel_relations(ChannelRelationsMode::Warn)
            .await
            .unwrap();
        assert_eq!(
            output
                .registrations
                .iter()
                .map(|r| r.registration.detector.as_source())
                .collect::<Vec<_>>(),
            ["discovered-detector", "independent-detector"]
        );
        assert_eq!(
            resolution(&output.registrations[0]),
            [url(&server, "discovered")]
        );
        assert!(
            output
                .warnings
                .iter()
                .any(|warning| matches!(warning, GatewayWarning::ChannelRelations(_)))
        );
        assert!(
            Gateway::new()
                .virtual_package_detectors(channels, [Subdir::Linux64, Subdir::NoArch])
                .channel_relations(ChannelRelationsMode::Strict)
                .await
                .is_err()
        );
        assert!(
            Gateway::new()
                .virtual_package_detectors(
                    [channel(&server, "discovered")],
                    [Subdir::Linux64, Subdir::NoArch]
                )
                .channel_relations(ChannelRelationsMode::Warn)
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn cycle_falls_back_to_the_registering_channel() {
        let dir = tempfile::tempdir().unwrap();
        let a = dir.path().join("a");
        let b = dir.path().join("b");
        write_subdir(
            &a,
            Subdir::Linux64,
            Some(r#"{"a-detect": ["__a"]}"#),
            Some("../b"),
            None,
        );
        write_subdir(&b, Subdir::Linux64, None, Some("../a"), None);

        let server = SimpleChannelServer::new(dir.path()).await;
        let output = Gateway::new()
            .virtual_package_detectors([channel(&server, "a")], [Subdir::Linux64, Subdir::NoArch])
            .await
            .unwrap();

        assert_eq!(output.registrations.len(), 1);
        assert_eq!(resolution(&output.registrations[0]), [url(&server, "a")]);
        assert!(output.warnings.iter().any(|w| matches!(
            w,
            GatewayWarning::VirtualPackageDetectors(
                VirtualPackageDetectorWarning::ResolutionFallback { .. }
            )
        )));
    }

    #[tokio::test]
    async fn disabled_relations_use_the_channel_alone() {
        let dir = tempfile::tempdir().unwrap();
        let forge = dir.path().join("conda-forge");
        let bio = dir.path().join("bioconda");
        write_subdir(
            &forge,
            Subdir::Linux64,
            Some(r#"{"cf-detect": ["__cf"]}"#),
            None,
            None,
        );
        write_subdir(
            &bio,
            Subdir::Linux64,
            Some(r#"{"bio-detect": ["__bio"]}"#),
            Some("../conda-forge"),
            None,
        );

        let server = SimpleChannelServer::new(dir.path()).await;
        let output = Gateway::new()
            .virtual_package_detectors(
                [channel(&server, "bioconda")],
                [Subdir::Linux64, Subdir::NoArch],
            )
            .channel_relations(ChannelRelationsMode::Disabled)
            .await
            .unwrap();
        assert_eq!(output.registrations.len(), 1);
        assert_eq!(names(&output.registrations[0]), ["__bio"]);
        assert_eq!(
            resolution(&output.registrations[0]),
            [url(&server, "bioconda")]
        );
    }

    #[tokio::test]
    async fn channels_without_registrations_yield_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("channel");
        write_subdir(&root, Subdir::Linux64, None, None, None);

        let server = SimpleChannelServer::new(dir.path()).await;
        let output = Gateway::new()
            .virtual_package_detectors(
                [channel(&server, "channel")],
                [Subdir::Linux64, Subdir::NoArch],
            )
            .await
            .unwrap();
        assert!(output.registrations.is_empty());
        assert!(output.rejected.is_empty());
        assert!(output.warnings.is_empty());
    }

    #[tokio::test]
    async fn query_detector_demand_uses_candidates_patches_and_explicit_constraints() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("channel");
        write_subdir(&root, Subdir::Linux64, None, None, None);
        write_subdir(&root, Subdir::NoArch, Some("null"), None, None);
        add_candidate(
            &root,
            Subdir::Linux64,
            "consumer",
            "1",
            &["__obsolete >=1"],
            &[],
        );
        add_candidate(
            &root,
            Subdir::Linux64,
            "consumer",
            "2",
            &[],
            &["__cuda >=12"],
        );
        let server = SimpleChannelServer::new(dir.path()).await;
        let disabled = Gateway::new()
            .query(
                [channel(&server, "channel")],
                [Subdir::Linux64],
                [PackageName::new_unchecked("consumer")],
            )
            .await
            .unwrap();
        assert!(disabled.virtual_package_detectors.is_none());
        assert_eq!(disabled.repodata[0].len(), 2);

        write_subdir(
            &root,
            Subdir::NoArch,
            Some(
                r#"{"mpi-detect":["__mpi"],"cuda-detect":["__cuda"],"extra-detect":["__extra"],"root-detect":["__root"],"idle-detect":["__idle"]}"#,
            ),
            None,
            None,
        );
        add_candidate(
            &root,
            Subdir::NoArch,
            "not-installed",
            "1",
            &["__unqueried"],
            &[],
        );
        write_subdir(
            &root,
            Subdir::Osx64,
            Some(r#"{"foreign-detect":["__foreign"]}"#),
            None,
            None,
        );
        let output = Gateway::new()
            .query(
                [channel(&server, "channel")],
                [Subdir::Linux64],
                [
                    rattler_conda_types::MatchSpec::from_str(
                        "consumer",
                        rattler_conda_types::ParseStrictness::Lenient,
                    )
                    .unwrap(),
                    rattler_conda_types::MatchSpec::from_str(
                        "__root",
                        rattler_conda_types::ParseStrictness::Lenient,
                    )
                    .unwrap(),
                ],
            )
            .constraints([
                rattler_conda_types::MatchSpec::from_str(
                    "__extra >=1",
                    rattler_conda_types::ParseStrictness::Lenient,
                )
                .unwrap(),
                rattler_conda_types::MatchSpec::from_str(
                    "not-installed >=1",
                    rattler_conda_types::ParseStrictness::Lenient,
                )
                .unwrap(),
            ])
            .virtual_package_detectors(Subdir::Linux64)
            .with_record_patch(|record| {
                if record.package_record.version.to_string() == "1" {
                    let mut record = record.clone();
                    record.package_record.depends = vec!["conda-forge::__mpi >=4".to_string()];
                    Some(record)
                } else {
                    None
                }
            })
            .await
            .unwrap();
        assert_eq!(output.repodata.len(), 1);
        assert_eq!(output.repodata[0].len(), 2);
        let detectors = output.virtual_package_detectors.unwrap();
        assert_eq!(detectors.target_platform, Subdir::Linux64);
        assert_eq!(
            detectors
                .wanted_names
                .iter()
                .map(PackageName::as_normalized)
                .collect::<Vec<_>>(),
            ["__cuda", "__extra", "__mpi", "__root"]
        );
        assert_eq!(
            detectors
                .registrations
                .iter()
                .map(|r| r.registration.detector.as_normalized())
                .collect::<BTreeSet<_>>(),
            BTreeSet::from([
                "mpi-detect",
                "cuda-detect",
                "extra-detect",
                "root-detect",
                "idle-detect"
            ])
        );
        assert!(detectors.rejected.is_empty());
        assert!(output.warnings.is_empty(), "{:?}", output.warnings);
    }

    #[tokio::test]
    async fn query_detector_constraints_work_without_record_specs_or_platforms() {
        let dir = tempfile::tempdir().unwrap();
        write_subdir(
            &dir.path().join("channel"),
            Subdir::NoArch,
            Some(r#"{"x-detect":["__x"]}"#),
            None,
            None,
        );
        let server = SimpleChannelServer::new(dir.path()).await;
        let output = Gateway::new()
            .query([channel(&server, "channel")], [], Vec::<PackageName>::new())
            .virtual_package_detectors(Subdir::Linux64)
            .constraints([rattler_conda_types::MatchSpec::from_str(
                "__x >=1",
                rattler_conda_types::ParseStrictness::Lenient,
            )
            .unwrap()])
            .await
            .unwrap();
        assert!(output.repodata.is_empty());
        let detectors = output.virtual_package_detectors.unwrap();
        assert_eq!(
            detectors
                .wanted_names
                .iter()
                .map(PackageName::as_normalized)
                .collect::<Vec<_>>(),
            ["__x"],
        );
        assert_eq!(
            detectors
                .registrations
                .iter()
                .map(|r| r.registration.detector.as_normalized())
                .collect::<Vec<_>>(),
            ["x-detect"],
        );
        assert!(output.warnings.is_empty());
    }

    #[tokio::test]
    async fn query_detector_discovery_reuses_separate_offline_sparse_snapshots() {
        let dir = tempfile::tempdir().unwrap();
        let snapshots = dir.path().join("snapshots");
        write_subdir(&snapshots, Subdir::Linux64, None, None, None);
        write_subdir(
            &snapshots,
            Subdir::NoArch,
            Some(r#"{"x-detect":["__x"]}"#),
            None,
            None,
        );
        let origin = Channel::from_url(
            url::Url::from_directory_path(dir.path().join("unavailable")).unwrap(),
        );
        let sources = [Subdir::Linux64, Subdir::NoArch].map(|platform| {
            crate::Source::SparseRepoData(vec![Arc::new(
                crate::sparse::SparseRepoData::from_file(
                    origin.clone(),
                    platform.as_str(),
                    snapshots.join(platform.as_str()).join("repodata.json"),
                    None,
                )
                .unwrap(),
            )])
        });
        let output = Gateway::new()
            .query(
                sources,
                [Subdir::Linux64, Subdir::NoArch],
                [PackageName::new_unchecked("__x")],
            )
            .virtual_package_detectors(Subdir::Linux64)
            .await
            .unwrap();
        let detectors = output.virtual_package_detectors.unwrap();
        assert_eq!(
            detectors
                .registrations
                .iter()
                .map(|r| r.registration.detector.as_normalized())
                .collect::<Vec<_>>(),
            ["x-detect"],
        );
        assert_eq!(detectors.registrations[0].origin(), &origin.base_url);
        assert!(output.warnings.is_empty());
    }

    #[tokio::test]
    async fn query_detector_metadata_only_preserves_multichannel_override_placement() {
        let dir = tempfile::tempdir().unwrap();
        for (name, registration, overrides) in [
            ("first", None, Some("../low")),
            ("second", Some(r#"{"second-detect":["__x"]}"#), None),
            ("low", Some(r#"{"low-detect":["__x"]}"#), None),
        ] {
            write_subdir(
                &dir.path().join(name),
                Subdir::NoArch,
                registration,
                None,
                overrides,
            );
        }
        let server = SimpleChannelServer::new(dir.path()).await;
        let group = crate::MultiSource::new(
            "group",
            vec![
                channel(&server, "first").into(),
                channel(&server, "second").into(),
            ],
        )
        .unwrap();
        let output = Gateway::new()
            .query([crate::Source::from(group)], [], Vec::<PackageName>::new())
            .virtual_package_detectors(Subdir::Linux64)
            .constraints([rattler_conda_types::MatchSpec::from_str(
                "__x",
                rattler_conda_types::ParseStrictness::Lenient,
            )
            .unwrap()])
            .await
            .unwrap();
        let detectors = output.virtual_package_detectors.unwrap();
        assert_eq!(
            detectors
                .registrations
                .iter()
                .map(|r| r.registration.detector.as_normalized())
                .collect::<Vec<_>>(),
            ["second-detect"],
        );
        assert_eq!(
            detectors
                .rejected
                .iter()
                .map(|r| r.registration.detector.as_normalized())
                .collect::<Vec<_>>(),
            ["low-detect"],
        );
        assert_eq!(
            detectors.rejected[0].conflict.accepted_channel,
            url(&server, "second")
        );
    }

    #[tokio::test]
    async fn query_detector_conflicts_preserve_multichannel_ownership_before_demand_filtering() {
        let dir = tempfile::tempdir().unwrap();
        for (name, registration, base, overrides) in [
            (
                "first",
                r#"{"first-detect":["__x"]}"#,
                Some("../base"),
                Some("../low"),
            ),
            (
                "second",
                r#"{"second-detect":["__x","__wanted"]}"#,
                None,
                None,
            ),
            ("base", r#"{"base-detect":["__x"]}"#, None, None),
            ("low", r#"{"low-detect":["__x"]}"#, None, None),
            ("third", r#"{"third-detect":["__third"]}"#, None, None),
        ] {
            write_subdir(
                &dir.path().join(name),
                Subdir::Linux64,
                Some(registration),
                base,
                overrides,
            );
            write_subdir(&dir.path().join(name), Subdir::NoArch, None, None, None);
        }
        let server = SimpleChannelServer::new(dir.path()).await;
        let group = crate::MultiSource::new(
            "group",
            vec![
                channel(&server, "first").into(),
                channel(&server, "second").into(),
            ],
        )
        .unwrap();
        let output = Gateway::new()
            .query(
                [
                    crate::Source::Multi(group),
                    channel(&server, "third").into(),
                ],
                [Subdir::Linux64, Subdir::NoArch],
                [PackageName::new_unchecked("__wanted")],
            )
            .virtual_package_detectors(Subdir::Linux64)
            .await
            .unwrap();
        let detectors = output.virtual_package_detectors.unwrap();
        assert_eq!(
            detectors
                .wanted_names
                .iter()
                .map(PackageName::as_normalized)
                .collect::<Vec<_>>(),
            ["__wanted"]
        );
        assert_eq!(
            detectors
                .registrations
                .iter()
                .map(|r| r.registration.detector.as_normalized())
                .collect::<Vec<_>>(),
            ["base-detect", "third-detect"]
        );
        assert_eq!(
            detectors
                .rejected
                .iter()
                .map(|r| r.registration.detector.as_normalized())
                .collect::<Vec<_>>(),
            ["first-detect", "second-detect", "low-detect"]
        );
        assert!(
            detectors
                .rejected
                .iter()
                .all(|r| r.conflict.accepted_detector.as_normalized() == "base-detect")
        );
    }
}
