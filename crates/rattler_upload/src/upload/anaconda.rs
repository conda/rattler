use std::borrow::Cow;

use fs_err::tokio as fs;
use miette::{IntoDiagnostic, miette};
use rattler_conda_types::PackageName;
use rattler_conda_types::package::AboutJson;
use reqwest::Client;
use reqwest::multipart::Form;
use reqwest::multipart::Part;
use serde::{Deserialize, Serialize};
use tracing::debug;
use tracing::info;
use url::Url;

use crate::url_with_trailing_slash::UrlWithTrailingSlash;

use super::VERSION;
use super::package::ExtractedPackage;

pub struct Anaconda {
    client: Client,
    url: UrlWithTrailingSlash,
}

#[derive(Serialize, Deserialize, Debug)]
struct PackageAttrs<'a> {
    package_types: Vec<String>,
    name: Cow<'a, PackageName>,
    #[serde(flatten)]
    about: Cow<'a, AboutJson>,
}

#[derive(Serialize, Deserialize, Debug)]
struct ReleaseCreationArgs<'a> {
    requirements: Vec<String>,
    announce: bool,
    description: Option<String>,
    #[serde(flatten)]
    about: Cow<'a, AboutJson>,
}

#[derive(Serialize, Deserialize, Debug)]
struct FileStageResponse {
    post_url: Url,
    form_data: serde_json::Map<String, serde_json::Value>,
    dist_id: String,
}

impl Anaconda {
    pub fn new(token: String, url: UrlWithTrailingSlash) -> Self {
        let mut default_headers = reqwest::header::HeaderMap::new();

        default_headers.append(
            "Accept",
            "application/json".parse().expect("failed to parse"),
        );
        default_headers.append(
            "Authorization",
            format!("token {}", token).parse().expect("failed to parse"),
        );

        default_headers.append(
            "x-binstar-api-version",
            "1.12.2".parse().expect("failed to parse"),
        );

        let client = Client::builder()
            .no_gzip()
            .user_agent(format!("rattler-build/{}", VERSION))
            .default_headers(default_headers)
            .build()
            .expect("failed to create client");

        Self { client, url }
    }
}

impl Anaconda {
    pub async fn create_or_update_package(
        &self,
        owner: &str,
        package: &ExtractedPackage<'_>,
    ) -> miette::Result<()> {
        let package_name = package.package_name();
        debug!("getting package {}/{}", owner, package_name.as_normalized(),);

        let url = self
            .url
            .join(&format!(
                "package/{}/{}",
                owner,
                package_name.as_normalized(),
            ))
            .into_diagnostic()?;

        let response = self
            .client
            .get(url)
            .send()
            .await
            .into_diagnostic()
            .map_err(|e| miette!("failed to send request: {}", e))?;

        let exists = match response.status() {
            reqwest::StatusCode::OK => true,
            reqwest::StatusCode::NOT_FOUND => false,
            _ => {
                return Err(miette!(
                    "failed to get existing package: {}",
                    response.status()
                ));
            }
        };

        let url = self
            .url
            .join(&format!(
                "package/{}/{}",
                owner,
                package_name.as_normalized(),
            ))
            .into_diagnostic()?;

        // See inspect_conda_info_dir in anaconda-client
        // https://github.com/Anaconda-Platform/anaconda-client/blob/master/binstar_client/inspect_package/conda.py#L81-L150
        // dumping the entire about.json as public_attrs seems to work fine
        let payload = serde_json::json!({
            "public": true,
            "publish": false,
            "public_attrs": PackageAttrs {
                package_types: vec!["conda".to_string()],
                name: Cow::Borrowed(package_name),
                about: Cow::Borrowed(package.about_json()),
            },
        });

        let req = if exists {
            debug!(
                "updating package {}/{}",
                owner,
                package_name.as_normalized(),
            );
            self.client.patch(url)
        } else {
            debug!(
                "creating package {}/{}",
                owner,
                package_name.as_normalized(),
            );
            self.client.post(url)
        };

        req.json(&payload)
            .send()
            .await
            .into_diagnostic()
            .map_err(|e| miette!("failed to send request: {}", e))?
            .error_for_status()
            .into_diagnostic()
            .map_err(|e| miette!("failed to create package: {}", e))?;

        Ok(())
    }

    pub async fn create_or_update_release(
        &self,
        owner: &str,
        package: &ExtractedPackage<'_>,
    ) -> miette::Result<()> {
        let package_name = package.package_name();
        let package_version = package.package_version();
        debug!(
            "getting release {}/{}/{}",
            owner,
            package_name.as_normalized(),
            package_version
        );

        let url = self
            .url
            .join(&format!(
                "release/{}/{}/{}",
                owner,
                package_name.as_normalized(),
                package_version,
            ))
            .into_diagnostic()?;

        let response = self
            .client
            .get(url)
            .send()
            .await
            .into_diagnostic()
            .map_err(|e| miette!("failed to send request: {}", e))?;

        let exists = match response.status() {
            reqwest::StatusCode::OK => true,
            reqwest::StatusCode::NOT_FOUND => false,
            _ => {
                return Err(miette!(
                    "failed to get existing release: {}",
                    response.status()
                ));
            }
        };

        let url = self
            .url
            .join(&format!(
                "release/{}/{}/{}",
                owner,
                package_name.as_normalized(),
                package_version,
            ))
            .into_diagnostic()?;

        let req = if exists {
            debug!(
                "updating release {}/{}/{}",
                owner,
                package_name.as_normalized(),
                package_version
            );
            self.client.patch(url).json(&serde_json::json!({
                "requirements": [],
                "announce": false,
                "description": null,
                "public_attrs": Cow::Borrowed(package.about_json())
            }))
        } else {
            debug!(
                "creating release {}/{}/{}",
                owner,
                package_name.as_normalized(),
                package_version
            );
            self.client.post(url).json(&ReleaseCreationArgs {
                requirements: vec![],
                announce: false,
                description: None,
                about: Cow::Borrowed(package.about_json()),
            })
        };

        req.send()
            .await
            .into_diagnostic()
            .map_err(|e| miette!("failed to send request: {}", e))?
            .error_for_status()
            .into_diagnostic()
            .map_err(|e| miette!("failed to create release: {}", e))?;

        Ok(())
    }

    pub async fn remove_file(
        &self,
        owner: &str,
        package: &ExtractedPackage<'_>,
    ) -> miette::Result<()> {
        let package_name = package.package_name();
        let package_version = package.package_version();
        let subdir = package
            .subdir()
            .ok_or(miette!("missing subdir in index.json"))?;
        let filename = package
            .filename()
            .ok_or(miette!("missing filename in index.json"))?;

        debug!(
            "removing file {}/{}/{}/{}/{}",
            owner,
            package_name.as_normalized(),
            package_version,
            subdir,
            filename,
        );

        let url = self
            .url
            .join(&format!(
                "dist/{}/{}/{}/{}/{}",
                owner,
                package_name.as_normalized(),
                package_version,
                subdir,
                filename,
            ))
            .into_diagnostic()?;

        self.client
            .delete(url)
            .send()
            .await
            .into_diagnostic()
            .map_err(|e| miette!("failed to send request: {}", e))?
            .error_for_status()
            .into_diagnostic()
            .map_err(|e| miette!("failed to remove file: {}", e))?;

        Ok(())
    }

    pub async fn upload_file(
        &self,
        owner: &str,
        channels: &[String],
        force: bool,
        package: &ExtractedPackage<'_>,
    ) -> miette::Result<bool> {
        if channels.is_empty() {
            return Err(miette!(
                "No channel selected - please specify at least one channel for upload to Anaconda.org"
            ));
        }

        let sha256 = package.sha256().into_diagnostic()?;

        let package_name = package.package_name();
        let version = package.package_version();

        let index_json = &package.index_json();

        let subdir = index_json
            .subdir
            .as_deref()
            .ok_or(miette!("missing subdir in index.json"))?;

        let filename = package.filename().ok_or(miette!("missing filename"))?;

        debug!(
            "uploading file {}/{}/{}/{}/{}",
            owner,
            package_name.as_normalized(),
            version,
            subdir,
            filename,
        );

        let url = self
            .url
            .join(&format!(
                "stage/{}/{}/{}/{}/{}",
                owner,
                package_name.as_normalized(),
                version,
                subdir,
                filename,
            ))
            .into_diagnostic()?;

        let payload = serde_json::json!({
            "distribution_type": "conda",
            "description": null,
            "attrs": index_json,
            "channels": channels,
            "sha256": sha256,
        });

        let resp = self
            .client
            .post(url)
            .json(&payload)
            .send()
            .await
            .into_diagnostic()
            .map_err(|e| miette!("failed to send request: {}", e))?;

        match resp.status() {
            reqwest::StatusCode::OK => (),
            reqwest::StatusCode::CONFLICT => {
                if force {
                    info!(
                        "file {} already exists, running with --force, removing file and retrying",
                        filename
                    );
                    self.remove_file(owner, package).await?;

                    // We cannot just retry the staging request here, because
                    // Anaconda might have garbage collected the release /
                    // package after the deletion of the file.
                    return Ok(false);
                } else {
                    return Err(miette!(
                        "file {} already exists, use --force to overwrite",
                        filename
                    ));
                }
            }
            _ => {
                return Err(miette!(
                    "failed to stage file, server replied with: {}",
                    resp.status()
                ));
            }
        }

        let parsed_response: FileStageResponse = resp
            .json()
            .await
            .into_diagnostic()
            .map_err(|e| miette!("failed to parse response: {}", e))?;

        debug!("Uploading file to S3 Bucket {}", parsed_response.post_url);

        let base64_md5 = package.base64_md5().into_diagnostic()?;
        let file_size = package.file_size().into_diagnostic()?;

        let mut form_data = Form::new();

        for (key, value) in parsed_response.form_data {
            let serde_json::Value::String(value) = value else {
                Err(miette!("invalid value in form data: {}", value))?
            };

            form_data = form_data.text(key, value);
        }

        let content = fs::read(package.path()).await.into_diagnostic()?;

        form_data = form_data.text("Content-Length", file_size.to_string());
        form_data = form_data.text("Content-MD5", base64_md5.to_string());
        form_data = form_data.part("file", Part::bytes(content));

        reqwest::Client::new()
            .post(parsed_response.post_url)
            .multipart(form_data)
            .header("Accept", "application/json")
            .send()
            .await
            .into_diagnostic()
            .map_err(|e| miette!("failed to send request: {}", e))?
            .error_for_status()
            .into_diagnostic()
            .map_err(|e| miette!("failed to upload file, server replied with: {}", e))?;

        debug!("Committing file {}", filename);

        let url = self
            .url
            .join(&format!(
                "commit/{}/{}/{}/{}/{}",
                owner,
                package_name.as_normalized(),
                version,
                subdir,
                filename,
            ))
            .into_diagnostic()?;

        self.client
            .post(url)
            .json(&serde_json::json!({
                "dist_id": parsed_response.dist_id,
            }))
            .send()
            .await
            .into_diagnostic()
            .map_err(|e| miette!("failed to send commit: {}", e))?
            .error_for_status()
            .into_diagnostic()
            .map_err(|e| miette!("failed to commit file, server replied with: {}", e))?;

        debug!("File {} uploaded successfully", filename);

        Ok(true)
    }
}
