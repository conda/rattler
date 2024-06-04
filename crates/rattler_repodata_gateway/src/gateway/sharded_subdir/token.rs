use crate::reporter::ResponseReporterExt;
use crate::Reporter;
use crate::{fetch::FetchRepoDataError, gateway::PendingOrFetched, GatewayError};
use chrono::{DateTime, TimeDelta, Utc};
use http::header::CACHE_CONTROL;
use http::HeaderValue;
use itertools::Either;
use parking_lot::Mutex;
use reqwest_middleware::ClientWithMiddleware;
use serde::{Deserialize, Serialize};
use std::ops::Add;
use std::sync::Arc;
use url::Url;

/// A simple client that makes it simple to fetch a token from the token endpoint.
pub struct TokenClient {
    client: ClientWithMiddleware,
    token_base_url: Url,
    token: Arc<Mutex<PendingOrFetched<Option<Arc<Token>>>>>,
    concurrent_request_semaphore: Arc<tokio::sync::Semaphore>,
}

impl TokenClient {
    pub fn new(
        client: ClientWithMiddleware,
        token_base_url: Url,
        concurrent_request_semaphore: Arc<tokio::sync::Semaphore>,
    ) -> Self {
        Self {
            client,
            token_base_url,
            token: Arc::new(Mutex::new(PendingOrFetched::Fetched(None))),
            concurrent_request_semaphore,
        }
    }

    /// Returns the current token or fetches a new one if the current one is expired.
    pub async fn get_token(
        &self,
        reporter: Option<&dyn Reporter>,
    ) -> Result<Arc<Token>, GatewayError> {
        let sender_or_receiver = {
            let mut token = self.token.lock();
            match &*token {
                PendingOrFetched::Fetched(Some(token)) if token.is_fresh() => {
                    // The token is still fresh.
                    return Ok(token.clone());
                }
                PendingOrFetched::Fetched(_) => {
                    let (sender, _) = tokio::sync::broadcast::channel(1);
                    let sender = Arc::new(sender);
                    *token = PendingOrFetched::Pending(Arc::downgrade(&sender));

                    Either::Left(sender)
                }
                PendingOrFetched::Pending(sender) => {
                    let sender = sender.upgrade();
                    if let Some(sender) = sender {
                        Either::Right(sender.subscribe())
                    } else {
                        let (sender, _) = tokio::sync::broadcast::channel(1);
                        let sender = Arc::new(sender);
                        *token = PendingOrFetched::Pending(Arc::downgrade(&sender));
                        Either::Left(sender)
                    }
                }
            }
        };

        let sender = match sender_or_receiver {
            Either::Left(sender) => sender,
            Either::Right(mut receiver) => {
                return match receiver.recv().await {
                    Ok(Some(token)) => Ok(token),
                    _ => {
                        // If this happens the sender was dropped.
                        Err(GatewayError::IoError(
                            "a coalesced request for a token failed".to_string(),
                            std::io::ErrorKind::Other.into(),
                        ))
                    }
                };
            }
        };

        let token_url = self
            .token_base_url
            .join("token")
            .expect("invalid token url");
        tracing::debug!("fetching token from {}", &token_url);

        // Fetch the token
        let token = {
            let _permit = self.concurrent_request_semaphore.acquire().await;
            let reporter = reporter.map(|r| (r, r.on_download_start(&token_url)));
            let response = self
                .client
                .get(token_url.clone())
                .header(CACHE_CONTROL, HeaderValue::from_static("max-age=0"))
                .send()
                .await
                .and_then(|r| r.error_for_status().map_err(Into::into))
                .map_err(GatewayError::from)?;

            let bytes = response
                .bytes_with_progress(reporter)
                .await
                .map_err(FetchRepoDataError::from)
                .map_err(GatewayError::from)?;

            if let Some((reporter, index)) = reporter {
                reporter.on_download_complete(&token_url, index);
            }

            let mut token: Token = serde_json::from_slice(&bytes).map_err(|e| {
                GatewayError::IoError("failed to parse sharded index token".to_string(), e.into())
            })?;

            // Ensure that the issued_at field is set.
            token.issued_at.get_or_insert_with(Utc::now);

            Arc::new(token)
        };

        // Reacquire the token
        let mut token_lock = self.token.lock();
        *token_lock = PendingOrFetched::Fetched(Some(token.clone()));

        // Publish the change
        let _ = sender.send(Some(token.clone()));

        Ok(token)
    }
}

/// The token endpoint response.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Token {
    pub token: Option<String>,
    issued_at: Option<DateTime<Utc>>,
    expires_in: Option<u64>,
    pub shard_base_url: Option<Url>,
}

impl Token {
    /// Returns true if the token is still considered to be valid.
    pub fn is_fresh(&self) -> bool {
        if let (Some(issued_at), Some(expires_in)) = (&self.issued_at, self.expires_in) {
            let now = Utc::now();
            if issued_at.add(TimeDelta::seconds(expires_in as i64)) > now {
                return false;
            }
        }
        true
    }

    /// Add the token to the headers if its available
    pub fn add_to_headers(&self, headers: &mut http::header::HeaderMap) {
        if let Some(token) = &self.token {
            headers.insert(
                http::header::AUTHORIZATION,
                HeaderValue::from_str(&format!("Bearer {token}")).unwrap(),
            );
        }
    }
}
