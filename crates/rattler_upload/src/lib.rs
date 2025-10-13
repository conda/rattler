pub mod upload;
pub(crate) mod utils;

use crate::upload::opt::{AnacondaOpts, ArtifactoryOpts, CondaForgeOpts, PrefixOpts};
use crate::utils::server_util::{
    check_server_type, extract_anaconda_info, extract_artifactory_info, extract_conda_forge_info,
    extract_prefix_info, extract_quetz_info, SimpleServerType,
};
use crate::utils::tool_configuration;
use miette::IntoDiagnostic;
use rattler_conda_types::package::ArchiveType;
use upload::opt::{
    AnacondaData, ArtifactoryData, CondaForgeData, PrefixData, QuetzData, QuetzOpts, ServerType,
    UploadOpts,
};

#[cfg(feature = "s3")]
use crate::upload::opt::{S3Data, S3Opts};
#[cfg(feature = "s3")]
use crate::utils::server_util::extract_s3_info;
#[cfg(feature = "s3")]
use rattler_s3::clap::{S3AddressingStyleOpts, S3CredentialsOpts};

/// Upload package to different channels
pub async fn upload_from_args(args: UploadOpts) -> miette::Result<()> {
    // Validate package files are provided
    if args.package_files.is_empty() {
        return Err(miette::miette!("No package files were provided."));
    }

    // Validate all files are conda packages
    for package_file in &args.package_files {
        if ArchiveType::try_from(package_file).is_none() {
            return Err(miette::miette!(
                "The file {} does not appear to be a conda package.",
                package_file.to_string_lossy()
            ));
        }
    }

    // Initialize authentication store
    let store = tool_configuration::get_auth_store(args.common.auth_file, args.auth_store)
        .into_diagnostic()?;

    // Check server type from host (if provided)
    let detected_type: SimpleServerType = match &args.host {
        Some(host_url) => check_server_type(host_url),
        None => SimpleServerType::Unknown,
    };

    // Use detected type if available, otherwise fall back to provided server_type
    let server_type = match detected_type {
        SimpleServerType::Unknown => {
            // If detection failed, use provided subcommand server_type or return error
            match args.server_type {
                Some(server_type) => server_type,
                None => {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::InvalidInput,
                        "Cannot determine server type from host and no server type provided",
                    ))
                    .into_diagnostic()
                }
            }
        }
        SimpleServerType::Quetz => {
            let host_url = args.host.as_ref().unwrap();
            let (base_url, channel) =
                extract_quetz_info(host_url).expect("Failed to parse Quetz URL");
            ServerType::Quetz(QuetzOpts {
                url: base_url,
                channels: channel,
                api_key: None,
            })
        }
        SimpleServerType::Artifactory => {
            let host_url = args.host.as_ref().unwrap();
            let (base_url, channel) =
                extract_artifactory_info(host_url).expect("Failed to parse Artifactory URL");
            ServerType::Artifactory(ArtifactoryOpts {
                url: base_url,
                channels: channel,
                username: None,
                password: None,
                token: None,
            })
        }
        SimpleServerType::Prefix => {
            let host_url = args.host.as_ref().unwrap();
            let (base_url, channel) =
                extract_prefix_info(host_url).expect("Failed to parse Prefix URL");
            ServerType::Prefix(PrefixOpts {
                url: base_url,
                channel,
                api_key: None,
                attestation: None,
                skip_existing: false,
            })
        }
        SimpleServerType::Anaconda => {
            let host_url = args.host.as_ref().unwrap();
            let (base_url, channel) =
                extract_anaconda_info(host_url).expect("Failed to parse Anaconda URL");
            ServerType::Anaconda(AnacondaOpts {
                url: Some(base_url),
                channels: Some(channel),
                api_key: None,
                owner: "".to_string(),
                force: false,
            })
        }
        #[cfg(feature = "s3")]
        SimpleServerType::S3 => {
            let host_url = args.host.as_ref().unwrap();
            let (endpoint_url, channel, region) =
                extract_s3_info(host_url).expect("Failed to parse S3 URL");
            ServerType::S3(S3Opts {
                channel,
                s3_credentials: S3CredentialsOpts {
                    endpoint_url: Some(endpoint_url),
                    region: Some(region),
                    access_key_id: None,
                    secret_access_key: None,
                    session_token: None,
                    addressing_style: S3AddressingStyleOpts::VirtualHost,
                    force_path_style: None,
                },
                credentials: None,
                force: false,
            })
        }
        SimpleServerType::CondaForge => {
            let host_url = args.host.as_ref().unwrap();
            let (base_url, channel) =
                extract_conda_forge_info(host_url).expect("Failed to parse Conda Forge URL");
            ServerType::CondaForge(CondaForgeOpts {
                anaconda_url: Some(base_url),
                staging_channel: Some(channel),
                staging_token: "".to_string(),
                feedstock: "".to_string(),
                feedstock_token: "".to_string(),
                validation_endpoint: None,
                provider: None,
                dry_run: false,
            })
        }
    };
    // Upload handler based on server type
    match server_type {
        ServerType::Quetz(quetz_opts) => {
            let quetz_data = QuetzData::from(quetz_opts);
            upload::upload_package_to_quetz(&store, &args.package_files, quetz_data).await
        }
        ServerType::Artifactory(artifactory_opts) => {
            let artifactory_data = ArtifactoryData::try_from(artifactory_opts)?;

            upload::upload_package_to_artifactory(&store, &args.package_files, artifactory_data)
                .await
        }
        ServerType::Prefix(prefix_opts) => {
            let prefix_data = PrefixData::from(prefix_opts);
            upload::upload_package_to_prefix(&store, &args.package_files, prefix_data).await
        }
        ServerType::Anaconda(anaconda_opts) => {
            let anaconda_data = AnacondaData::from(anaconda_opts);
            upload::upload_package_to_anaconda(&store, &args.package_files, anaconda_data).await
        }
        #[cfg(feature = "s3")]
        ServerType::S3(s3_opts) => {
            let s3_data = S3Data::from(s3_opts);
            upload::upload_package_to_s3(
                &store,
                s3_data.channel,
                s3_data.credentials,
                &args.package_files,
                s3_data.force, // force parameter - using false as default
            )
            .await
        }
        ServerType::CondaForge(conda_forge_opts) => {
            let conda_forge_data = CondaForgeData::from(conda_forge_opts);
            upload::conda_forge::upload_packages_to_conda_forge(
                &args.package_files,
                conda_forge_data,
            )
            .await
        }
    }
}
