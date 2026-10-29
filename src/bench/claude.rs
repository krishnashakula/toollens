//! Minimal Claude Messages API client over raw HTTP (there is no official Rust SDK).

use std::time::Duration;

use anyhow::{Context, bail};
use serde_json::Value;

const API: &str = "https://api.anthropic.com/v1";
const VERSION: &str = "2026-10-21";
const MAX_ATTEMPTS: u32 = 6;

pub struct Claude {
    http: reqwest::Client,
    key: String,
}

impl Claude {
    pub fn from_env() -> anyhow::Result<Self> {
        let key = std::env::var("ANTHROPIC_API_KEY").context("ANTHROPIC_API_KEY is not set")?;
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(600))
            .build()?;
        Ok(Self { http, key })
    }

    pub async fn messages(&self, body: &Value) -> anyhow::Result<Value> {
        self.post("messages", body).await
    }

    pub async fn count_tokens(&self, body: &Value) -> anyhow::Result<u64> {
        let v = self.post("messages/count_tokens", body).await?;
        v["input_tokens"]
            .as_u64()
            .context("count_tokens response missing input_tokens")
    }

    /// Retries rate limits, overload, 5xx and connection errors with backoff (honoring
    /// `retry-after`); any other status fails immediately with the API's error message.
    async fn post(&self, path: &str, body: &Value) -> anyhow::Result<Value> {
        let mut attempt = 0;
        loop {
            attempt += 1;
            let backoff = Duration::from_secs(2u64.pow(attempt).min(60));
            let resp = self
                .http
                .post(format!("{API}/{path}"))
                .header("x-api-key", &self.key)
                .header("anthropic-version", VERSION)
                .json(body)
                .send()
                .await;
            let resp = match resp {
                Ok(r) => r,
                Err(e) if attempt < MAX_ATTEMPTS => {
                    tracing::warn!(error = %e, attempt, "Claude API connection error, retrying");
                    tokio::time::sleep(backoff).await;
                    continue;
                }
                Err(e) => return Err(e).context("Claude API request failed"),
            };
            let status = resp.status();
            let retry_after = resp
                .headers()
                .get("retry-after")
                .and_then(|v| v.to_str().ok())
                .and_then(|v| v.parse::<u64>().ok())
                .map(|s| Duration::from_secs(s.min(120)));
            let json = match resp.json::<Value>().await {
                Ok(v) => v,
                // A 2xx whose body could not be read is a failure, not an empty answer.
                Err(e) if status.is_success() => {
                    return Err(e).context("Claude API response body could not be read");
                }
                Err(_) => Value::Null,
            };
            if status.is_success() {
                return Ok(json);
            }
            let retryable = matches!(status.as_u16(), 429 | 500 | 502 | 503 | 504 | 529);
            if retryable && attempt < MAX_ATTEMPTS {
                let wait = retry_after.unwrap_or(backoff);
                tracing::warn!(%status, attempt, wait_s = wait.as_secs(), "Claude API retryable error");
                tokio::time::sleep(wait).await;
                continue;
            }
            let msg = json["error"]["message"]
                .as_str()
                .unwrap_or("no error message");
            bail!("Claude API {status}: {msg}");
        }
    }
}
