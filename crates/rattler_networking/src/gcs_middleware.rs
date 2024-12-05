//! Middleware to handle `gcs://` URLs to pull artifacts from an GCS
use async_trait::async_trait;
use google_cloud_auth::{project::Config, token::DefaultTokenSourceProvider};
use google_cloud_token::TokenSourceProvider;
use reqwest::{Request, Response};
use reqwest_middleware::{Middleware, Next, Result as MiddlewareResult};
use url::Url;

/// GCS middleware to authenticate requests
pub struct GCSMiddleware;

#[async_trait]
impl Middleware for GCSMiddleware {
    /// Create a new authentication middleware for GCS
    async fn handle(
        &self,
        mut req: Request,
        extensions: &mut http::Extensions,
        next: Next<'_>,
    ) -> MiddlewareResult<Response> {
        if req.url().scheme() == "gcs" {
            let mut url = req.url().clone();
            let bucket_name = url.host_str().expect("Host should be present in GCS URL");
            let new_url = format!(
                "https://storage.googleapis.com/{}{}",
                bucket_name,
                url.path()
            );
            url = Url::parse(&new_url).expect("Failed to parse URL");
            *req.url_mut() = url;
            req = authenticate_with_google_cloud(req).await?;
        }
        next.run(req, extensions).await
    }
}

/// Auth to GCS
async fn authenticate_with_google_cloud(mut req: Request) -> MiddlewareResult<Request> {
    let scopes = ["https://www.googleapis.com/auth/devstorage.read_only"];

    let config = Config::default().with_scopes(&scopes);

    match DefaultTokenSourceProvider::new(config).await {
        Ok(provider) => match provider.token_source().token().await {
            Ok(token) => {
                let header_value = reqwest::header::HeaderValue::from_str(&token)
                    .map_err(reqwest_middleware::Error::middleware)?;
                req.headers_mut()
                    .insert(reqwest::header::AUTHORIZATION, header_value);
                Ok(req)
            }
            Err(e) => Err(reqwest_middleware::Error::Middleware(anyhow::anyhow!(
                "Failed to get GCS token: {:?}",
                e
            ))),
        },
        Err(e) => Err(reqwest_middleware::Error::Middleware(anyhow::Error::new(e))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use reqwest::Client;
    use tempfile;

    #[tokio::test]
    async fn test_gcs_middleware() {
        let credentials = match std::env::var("GOOGLE_CLOUD_TEST_KEY_JSON") {
            Ok(credentials) if !credentials.is_empty() => credentials,
            Err(_) | Ok(_) => {
                eprintln!("Skipping test as GOOGLE_CLOUD_TEST_KEY_JSON is not set");
                return;
            }
        };
        println!("Running GCS Test");

        // We have to set GOOGLE_APPLICATION_CREDENTIALS to the path of the JSON key file
        let key_file = tempfile::NamedTempFile::with_suffix(".json").unwrap();
        std::fs::write(&key_file, credentials).unwrap();

        let prev_value = std::env::var("GOOGLE_APPLICATION_CREDENTIALS").ok();
        std::env::set_var("GOOGLE_APPLICATION_CREDENTIALS", key_file.path());

        let client = reqwest_middleware::ClientBuilder::new(Client::new())
            .with(GCSMiddleware)
            .build();

        let url = "gcs://test-channel/noarch/repodata.json";
        let response = client.get(url).send().await.unwrap();
        assert!(response.status().is_success());

        let url = "gcs://test-channel-nonexist/noarch/repodata.json";
        let response = client.get(url).send().await.unwrap();
        assert!(response.status().is_client_error());

        if let Some(value) = prev_value {
            std::env::set_var("GOOGLE_APPLICATION_CREDENTIALS", value);
        } else {
            std::env::remove_var("GOOGLE_APPLICATION_CREDENTIALS");
        }
    }
}
