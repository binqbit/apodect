//! Bounded JSON-RPC transport. Retries belong to the operation that knows whether
//! a request is a read, an exact rebroadcast, or a new financial effect.

use std::{
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};

use anyhow::{Context, Result, ensure};
use reqwest::{Client, Url, redirect::Policy};
use serde::de::DeserializeOwned;
use serde_json::{Value, json};

const MAX_RESPONSE_BYTES: usize = 8_000_000;

#[derive(Clone)]
pub struct Rpc {
    client: Client,
    url: Url,
    next_id: Arc<AtomicU64>,
}

#[derive(Debug)]
pub struct RpcError {
    pub code: i64,
    pub message: String,
    pub data: Option<Value>,
    pub cause: Option<Value>,
}

impl std::fmt::Display for RpcError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "RPC error {}: {}", self.code, self.message)
    }
}

impl std::error::Error for RpcError {}

impl Rpc {
    pub fn new(url: &str) -> Result<Self> {
        let url = Url::parse(url).context("Invalid RPC URL")?;
        ensure!(
            matches!(url.scheme(), "http" | "https"),
            "RPC URL must use HTTP or HTTPS"
        );
        ensure!(
            url.fragment().is_none(),
            "RPC URL must not contain a fragment"
        );
        let client = Client::builder()
            .redirect(Policy::none())
            .connect_timeout(Duration::from_secs(5))
            .timeout(Duration::from_secs(30))
            .build()?;

        Ok(Self {
            client,
            url,
            next_id: Arc::new(AtomicU64::new(1)),
        })
    }

    pub async fn call<T: DeserializeOwned>(&self, method: &str, params: &Value) -> Result<T> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let mut response = self
            .client
            .post(self.url.clone())
            .json(&json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}))
            .send()
            .await
            .map_err(|error| error.without_url())?
            .error_for_status()
            .map_err(|error| error.without_url())?;
        ensure!(
            response.content_length().unwrap_or(0) <= MAX_RESPONSE_BYTES as u64,
            "RPC response exceeds size limit"
        );

        let mut bytes = Vec::new();
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|error| error.without_url())?
        {
            ensure!(
                bytes.len() + chunk.len() <= MAX_RESPONSE_BYTES,
                "RPC response exceeds size limit"
            );
            bytes.extend_from_slice(&chunk);
        }
        let envelope: Value = serde_json::from_slice(&bytes).context("Invalid RPC JSON")?;
        ensure!(
            envelope["jsonrpc"] == "2.0" && envelope["id"] == id,
            "RPC response identity mismatch"
        );
        if let Some(error) = envelope.get("error") {
            ensure!(envelope.get("result").is_none(), "Ambiguous RPC response");
            return Err(RpcError {
                code: error["code"].as_i64().context("Missing RPC error code")?,
                message: error["message"]
                    .as_str()
                    .context("Missing RPC error message")?
                    .to_owned(),
                cause: error.get("cause").cloned(),
                data: error.get("data").or_else(|| error.get("cause")).cloned(),
            }
            .into());
        }
        let result = envelope.get("result").context("Missing RPC result")?;
        serde_json::from_value(result.clone()).with_context(|| format!("Invalid {method} response"))
    }
}
