//! Middleware to handle `gcs://` URLs to pull artifacts from an GCS
use async_trait::async_trait;
use google_cloud_auth::project::{create_token_source, Config};
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
    let audience = "https://storage.googleapis.com/";
    let scopes = [
        "https://www.googleapis.com/auth/cloud-platform",
        "https://www.googleapis.com/auth/devstorage.read_only",
    ];
    match create_token_source(Config {
        audience: Some(audience),
        scopes: Some(&scopes),
        sub: None,
    })
    .await
    {
        Ok(ts) => match ts.token().await {
            Ok(token) => {
                let bearer_auth = format!("Bearer {}", token.access_token);
                let header_value = reqwest::header::HeaderValue::from_str(&bearer_auth)
                    .map_err(reqwest_middleware::Error::middleware)?;
                req.headers_mut()
                    .insert(reqwest::header::AUTHORIZATION, header_value);
                Ok(req)
            }
            Err(e) => Err(reqwest_middleware::Error::Middleware(anyhow::Error::new(e))),
        },
        Err(e) => Err(reqwest_middleware::Error::Middleware(anyhow::Error::new(e))),
    }
}
