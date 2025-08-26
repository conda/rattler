pub mod upload;
pub(crate) mod utils;

use miette::IntoDiagnostic;
use rattler_conda_types::package::ArchiveType;
use upload::opt::{
    AnacondaData, ArtifactoryData, CondaForgeData, PrefixData, QuetzData, ServerType, UploadOpts,
};

use crate::utils::tool_configuration;
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

    // Upload handler based on server type
    match args.server_type {
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
            upload::upload_package_to_s3(
                &store,
                s3_opts.channel,
                s3_opts.endpoint_url,
                s3_opts.region,
                s3_opts.force_path_style,
                s3_opts.access_key_id,
                s3_opts.secret_access_key,
                s3_opts.session_token,
                &args.package_files,
                s3_opts.force,
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
