use std::{
    collections::HashSet,
    io::{IsTerminal, Write},
};

use miette::{Context, IntoDiagnostic};
use rattler::package_cache::PackageCache;
use rattler_conda_types::{Channel, GenericVirtualPackage, VirtualPackageName};
use rattler_config::{
    ConfigBase, NoExtension, config::virtual_package_detectors::DetectorDecision,
};
use rattler_repodata_gateway::{Gateway, QueryVirtualPackageDetectors};
use rattler_virtual_package_detectors::{
    CacheClock, ConfiguredConsent, DenyAll, DetectOptions, EnvironmentOptions, EnvironmentSnapshot,
    RattlerEnvironmentProvider, SkipReason, WantedNames, detect, merge_results, read_override,
};
use reqwest_middleware::ClientWithMiddleware;

use crate::solver_args::SolverArgs;

pub(super) struct DetectorContext<'a> {
    pub gateway: &'a Gateway,
    pub config: &'a ConfigBase<NoExtension>,
    pub download_client: &'a ClientWithMiddleware,
    pub detectors: Option<QueryVirtualPackageDetectors>,
}

pub(super) async fn detect_virtual_packages(
    solver: &SolverArgs,
    context: DetectorContext<'_>,
) -> miette::Result<Vec<GenericVirtualPackage>> {
    // --virtual-package has always replaced the complete automatic capability set.
    let mut virtual_packages = solver.virtual_packages()?;
    if solver.has_explicit_virtual_packages() {
        return Ok(virtual_packages);
    }

    let Some(discovery) = context.detectors else {
        return Ok(virtual_packages);
    };
    if discovery.wanted_names.is_empty() || discovery.registrations.is_empty() {
        return Ok(virtual_packages);
    }

    let host_platform = crate::host_platform()?;
    let environment = EnvironmentSnapshot::from_system();
    let mut detector_config = context.config.virtual_package_detectors.clone();
    if discovery.target_platform == host_platform {
        for registration in &discovery.registrations {
            let mut needs_run = false;
            for name in &registration.registration.virtual_packages {
                if discovery.wanted_names.contains(name.as_package_name())
                    && read_override(name, &environment)
                        .into_diagnostic()?
                        .is_none()
                {
                    needs_run = true;
                }
            }
            if !needs_run || detector_config.consent(registration.origin()).is_some() {
                continue;
            }
            let decision = channel_consent(&registration.channel)?;
            detector_config
                .consent
                .insert(registration.origin().clone(), decision);
        }
    }
    let timeout = detector_config
        .timeout()
        .unwrap_or(rattler_virtual_package_detectors::limits::DEFAULT_TIMEOUT);
    let consent = ConfiguredConsent::new(detector_config, DenyAll);
    let root = rattler::default_cache_dir()
        .map_err(|error| miette::miette!("could not determine default cache directory: {error}"))?
        .join("virtual-package-detectors");
    let environments = root.join("environments");
    let package_cache = PackageCache::new(root.join("packages"));
    let provider = RattlerEnvironmentProvider::new(EnvironmentOptions {
        gateway: context.gateway,
        package_cache: &package_cache,
        download_client: context.download_client.clone().into(),
        root: &environments,
        host_platform,
        virtual_packages: if discovery.target_platform == host_platform {
            virtual_packages.clone()
        } else {
            // The engine never resolves a detector for a foreign target.
            Vec::new()
        },
    });
    let outcome = detect(
        &discovery.registrations,
        DetectOptions {
            environment_provider: &provider,
            environment: &environment,
            root: &root,
            host_platform,
            target_platform: discovery.target_platform,
            timeout,
            consent: &consent,
            wanted: WantedNames::Only(discovery.wanted_names),
            concurrency: context.config.concurrency.solves,
            clock: CacheClock::current(),
        },
    )
    .await
    .into_diagnostic()?;
    for failure in &outcome.failures {
        eprintln!(
            "warning: virtual package detector '{}' from {} failed: {}",
            failure.detector.as_normalized(),
            failure.origin,
            failure.error,
        );
        if let Some(stderr) = &failure.stderr
            && !stderr.is_empty()
        {
            eprintln!("{stderr}");
        }
    }
    for skipped in &outcome.skipped {
        if let SkipReason::TargetIsNotHost { override_variables } = &skipped.reason {
            eprintln!(
                "warning: not running host detector '{}' for foreign target {}; use {} to override its capabilities",
                skipped.detector.as_normalized(),
                discovery.target_platform,
                override_variables.join(", "),
            );
        }
    }
    // An accepted registration owns its names, including when denied or failed.
    // Never fall back to builtin detection for a name assigned to that detector.
    let registered: HashSet<_> = discovery
        .registrations
        .iter()
        .flat_map(|registration| {
            registration
                .registration
                .virtual_packages
                .iter()
                .map(VirtualPackageName::as_package_name)
        })
        .collect();
    virtual_packages.retain(|package| !registered.contains(&package.name));
    Ok(merge_results(virtual_packages, &outcome.results))
}

fn channel_consent(channel: &Channel) -> miette::Result<DetectorDecision> {
    if !std::io::stdin().is_terminal() || !std::io::stderr().is_terminal() {
        eprintln!(
            "warning: skipping virtual package detectors from {}: no stored consent and no interactive terminal; configure virtual-package-detectors.consent for this channel to allow or deny execution",
            channel.base_url,
        );
        return Ok(DetectorDecision::Deny);
    }

    eprintln!(
        "Channel {} registers virtual package detectors. Trusting it allows installing and executing all its current and future detectors and their resolved dependencies in isolated environments. This decision is shared with other rattler-based tools, not limited to the target prefix.",
        channel.base_url,
    );
    eprint!("Trust this channel's detectors? [y/N] ");
    std::io::stderr().flush().into_diagnostic()?;
    let mut answer = String::new();
    std::io::stdin().read_line(&mut answer).into_diagnostic()?;
    let decision = if matches!(answer.trim().to_ascii_lowercase().as_str(), "y" | "yes") {
        DetectorDecision::Allow
    } else {
        DetectorDecision::Deny
    };

    let path = rattler_config::locations::shared_user_config_paths()
        .pop()
        .ok_or_else(|| miette::miette!("could not determine shared user configuration path"))?;
    // Edit only this shared file, not the merged configuration from all layers.
    let mut config = if path.exists() {
        ConfigBase::<NoExtension>::load_from_files([&path]).into_diagnostic()?
    } else {
        ConfigBase::<NoExtension>::default()
    };
    config
        .virtual_package_detectors
        .consent
        .insert(channel.base_url.clone(), decision);
    config
        .save(&path)
        .into_diagnostic()
        .context("failed to persist detector channel consent")?;
    Ok(decision)
}
