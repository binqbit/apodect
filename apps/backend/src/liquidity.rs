//! Exact-output quotes prepare an authority-signed Apply intent. Provider status
//! describes transport progress; only native receipts can credit Project funds.

use std::time::Duration;

use alloy_primitives::Address;
use anyhow::{Context, Result, ensure};
use chrono::{DateTime, SecondsFormat, Utc};
use ed25519_dalek::{Signature, VerifyingKey};
use near_primitives::types::AccountId;
use rand::{RngCore, rngs::OsRng};
use reqwest::{Client, RequestBuilder, Url, redirect::Policy};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use crate::{
    chains::{
        near::{FunctionCallIntent, RouteQuote},
        protocol::canonical_json,
    },
    config::{Chain, Config, DomainConfig, LiquidityConfig, ProjectConfig, word},
    indexing::Indexer,
};

const MAX_RESPONSE_BYTES: usize = 1_000_000;
const APPLY_DEPOSIT: u128 = 100_000_000_000_000_000_000_000;
// Production manager key published by the official 1Click SDK.
const ONE_CLICK_KEY: &str = "reYaWhvwu8Jzo3WUM3zhn6VrhuMEF4eADL17qtRVifc";
const ETHEREUM_MAINNET_GENESIS: &str =
    "d4e56740f876aef8c010b86a40d5f56745a118d0906a34e69aec8c0db1cb8fa3";
const SOLANA_MAINNET_GENESIS: &str = "5eykt4UsFv8P8NJdTREpY1vzqKqZKvdpKuc147dw2N9d";
// Native mainnet USDC identities published by Circle, independent of provider symbols.
const ETHEREUM_USDC: &str = "0xA0b86991c6218b36c1d19D4a2e9Eb0cE3606eB48";
const SOLANA_USDC: &str = "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v";
const NEAR_USDC: &str = "17208628f84f5d6ad33f0da3bbbeb27ffcb398eac501a31bd6ad2011e36133a1";

pub struct Liquidity {
    client: Client,
    url: Url,
    local_simulator: bool,
    api_key: Option<String>,
    fee_warning_bps: u16,
}

#[derive(Debug, Serialize)]
#[serde(tag = "state")]
pub enum FundingProposal {
    Waiting {
        #[serde(with = "crate::decimal_u64")]
        shortfall: u64,
        reason: FundingWaitReason,
    },
    Ready {
        authority: String,
        call: FunctionCallIntent,
        costs: FundingCosts,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FundingWaitReason {
    InsufficientLiquidity,
    ProviderQuoteUnavailable,
}

enum QuoteAvailability {
    Affordable,
    InsufficientLiquidity,
    ProviderDeclined,
}

#[derive(Debug, Serialize)]
pub struct FundingCosts {
    #[serde(with = "crate::decimal_u64")]
    pub principal: u64,
    #[serde(with = "crate::decimal_u64")]
    pub local_amount: u64,
    #[serde(with = "crate::decimal_u64")]
    pub expected_transport_cost: u64,
    #[serde(with = "crate::decimal_u64")]
    pub maximum_transport_cost: u64,
    pub warnings: Vec<FeeWarning>,
}

#[derive(Debug, Serialize)]
pub struct FeeWarning {
    pub route: String,
    #[serde(with = "crate::decimal_u64")]
    pub fee: u64,
    pub threshold_bps: u16,
}

impl FundingCosts {
    fn from_routes(
        principal: u64,
        local_amount: u64,
        routes: &[RouteQuote],
        threshold: u16,
    ) -> Result<Self> {
        let mut costs = Self {
            principal,
            local_amount,
            expected_transport_cost: 0,
            maximum_transport_cost: 0,
            warnings: Vec::new(),
        };
        for route in routes {
            let expected = route
                .min_input
                .checked_sub(route.output)
                .context("Invalid quoted fee")?;
            let maximum = route
                .input
                .checked_sub(route.output)
                .context("Invalid quoted fee buffer")?;
            costs.expected_transport_cost = costs
                .expected_transport_cost
                .checked_add(expected)
                .context("Quoted fees overflow")?;
            costs.maximum_transport_cost = costs
                .maximum_transport_cost
                .checked_add(maximum)
                .context("Quoted fee buffers overflow")?;
            if u128::from(expected) * 10_000 > u128::from(route.output) * u128::from(threshold) {
                costs.warnings.push(FeeWarning {
                    route: route.route.clone(),
                    fee: expected,
                    threshold_bps: threshold,
                });
            }
        }
        Ok(costs)
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct QuoteRequest {
    pub dry: bool,
    pub swap_type: String,
    pub slippage_tolerance: u16,
    pub origin_asset: String,
    pub destination_asset: String,
    pub deposit_type: String,
    pub recipient_type: String,
    pub refund_type: String,
    pub recipient: String,
    pub refund_to: String,
    pub amount: String,
    pub deadline: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub correlation_id: Option<String>,
}

#[derive(Debug)]
pub struct Quote {
    pub response: Value,
    pub input: u64,
    pub minimum_input: u64,
    pub output: u64,
    pub deposit_address: Option<String>,
    pub expires: Option<u64>,
}

impl Liquidity {
    pub fn new(config: &LiquidityConfig) -> Result<Self> {
        let url = Url::parse(&format!("{}/", config.url.trim_end_matches('/')))?;
        ensure!(
            url.username().is_empty()
                && url.password().is_none()
                && url.query().is_none()
                && url.fragment().is_none(),
            "Invalid liquidity provider URL"
        );
        ensure!(
            url.scheme() == "https" || (config.local_simulator && url.scheme() == "http"),
            "Public liquidity providers require HTTPS"
        );
        let api_key = config
            .api_key_file
            .as_ref()
            .map(|path| {
                std::fs::read_to_string(path)
                    .context("Cannot read liquidity provider credential")
                    .map(|value| value.trim_end_matches(['\r', '\n']).to_owned())
            })
            .transpose()?;
        ensure!(
            api_key.as_ref().is_none_or(|value| !value.is_empty()),
            "Liquidity API key is empty"
        );
        let fee_warning_bps = std::env::var("APODECT_FEE_WARNING_BPS")
            .unwrap_or_else(|_| "500".into())
            .parse::<u16>()
            .context("APODECT_FEE_WARNING_BPS must be between 0 and 10000")?;
        ensure!(
            fee_warning_bps <= 10_000,
            "APODECT_FEE_WARNING_BPS must be between 0 and 10000"
        );
        let client = Client::builder()
            .redirect(Policy::none())
            .connect_timeout(Duration::from_secs(5))
            .timeout(Duration::from_secs(30))
            .build()?;
        Ok(Self {
            client,
            url,
            local_simulator: config.local_simulator,
            api_key,
            fee_warning_bps,
        })
    }

    pub async fn quote(&self, request: &QuoteRequest) -> Result<Quote> {
        let expected = serde_json::to_value(request)?;
        let response = self
            .request(self.client.post(self.url.join("v0/quote")?).json(&expected))
            .await?;
        if !self.local_simulator {
            verify_quote_signature(&response, ONE_CLICK_KEY)?;
        }
        validate_quote(request, response, self.local_simulator)
    }

    async fn affordable_quote(
        &self,
        request: &mut QuoteRequest,
        available: u64,
    ) -> Result<QuoteAvailability> {
        ensure!(
            request.dry,
            "Affordability probes must not allocate deposit addresses"
        );
        for _ in 0..64 {
            let quote = match self.quote(request).await {
                Ok(quote) => quote,
                Err(error)
                    if error.downcast_ref::<reqwest::Error>().is_some_and(|error| {
                        error.status() == Some(reqwest::StatusCode::BAD_REQUEST)
                    }) =>
                {
                    // A provider may decline a small/unsupported quote with HTTP 400.
                    // Its prose is not a machine contract, and no funds are reserved.
                    return Ok(QuoteAvailability::ProviderDeclined);
                }
                Err(error) => return Err(error),
            };
            if quote.input <= available {
                return Ok(QuoteAvailability::Affordable);
            }
            // This only chooses the next amount to ask about. The provider must quote
            // it again; a proportional estimate never establishes affordability.
            let output =
                (u128::from(quote.output) * u128::from(available)) / u128::from(quote.input);
            if output == 0 {
                return Ok(QuoteAvailability::InsufficientLiquidity);
            }
            request.amount = u64::try_from(output)?.to_string();
        }
        Ok(QuoteAvailability::InsufficientLiquidity)
    }

    /// This response is diagnostic only; it cannot settle dispatch, delivery or refund accounting.
    pub async fn status(&self, quote_response: &Value) -> Result<Value> {
        let quote = &quote_response["quote"];
        let address = quote["depositAddress"]
            .as_str()
            .context("Quote has no deposit address")?;
        let mut url = self.url.join("v0/status")?;
        url.query_pairs_mut().append_pair("depositAddress", address);
        if let Some(memo) = quote["depositMemo"].as_str() {
            url.query_pairs_mut().append_pair("depositMemo", memo);
        }
        let response = self.request(self.client.get(url)).await?;
        if !self.local_simulator {
            let returned = &response["quoteResponse"];
            verify_quote_signature(returned, ONE_CLICK_KEY)?;
            ensure!(
                signed_quote_hash(returned)? == signed_quote_hash(quote_response)?,
                "Status references a different quote"
            );
        }
        Ok(response)
    }

    pub async fn propose_apply(
        &self,
        indexer: &Indexer,
        project: &ProjectConfig,
        destination: Chain,
        request_id: &str,
        slippage_bps: u16,
        expires: u64,
    ) -> Result<FundingProposal> {
        ensure!(
            slippage_bps <= 1_000,
            "Slippage exceeds the ten-percent limit"
        );
        let request_id = hex::encode(word(request_id)?);
        let view = indexer.index_project(project).await?;
        let destination_binding = &project.domains[&destination];
        let request: ApprovedRequest = indexer
            .near
            .view(
                &project.coordinator,
                "get_request",
                &json!({"domain": destination_binding.domain, "request": request_id}),
                &view.block,
            )
            .await?;
        ensure!(
            request.status == "Queued"
                && request.local_state == 0
                && request.approval_position == Some(view.state.queue_head),
            "Request is not the available FIFO head"
        );
        ensure!(request.amount > 0, "Request amount is zero");
        ensure!(expires > now()?, "Apply deadline has expired");

        let local_amount = request.amount.min(view.domains[&destination].available);
        let required = request.amount - local_amount;
        let candidates = if required == 0 {
            Vec::new()
        } else {
            self.validate_assets(&indexer.config).await?;
            route_candidates(
                indexer,
                project,
                destination,
                &view.domains,
                slippage_bps,
                expires,
            )
            .await?
        };
        let (shortfall, quotes, reason) = self.quote_routes(candidates, required).await?;
        if shortfall > 0 {
            return Ok(FundingProposal::Waiting { shortfall, reason });
        }

        let mut routes = Vec::with_capacity(quotes.len());
        for quote in quotes {
            indexer
                .store
                .save_quote(
                    &indexer.config.project_key(project),
                    &quote.route.route,
                    &quote.response,
                )
                .await?;
            routes.push(quote.route);
        }
        ensure!(expires > now()?, "Quotes expired while preparing Apply");
        let costs =
            FundingCosts::from_routes(request.amount, local_amount, &routes, self.fee_warning_bps)?;
        Ok(FundingProposal::Ready {
            authority: view.state.authority_id,
            costs,
            call: FunctionCallIntent {
                receiver_id: project.coordinator.clone(),
                method_name: "apply".into(),
                args: json!({"domain": destination_binding.domain, "request": request_id,
                    "local_amount": local_amount, "routes": routes, "expires": expires}),
                gas: 300_000_000_000_000,
                deposit: APPLY_DEPOSIT,
            },
        })
    }

    /// Admit configured public assets against one current provider catalog read.
    /// The explicit local simulator does not use the public catalog or mainnet.
    pub async fn validate_assets(&self, config: &Config) -> Result<()> {
        if self.local_simulator {
            return Ok(());
        }
        validate_mainnet(config)?;
        let response = self
            .request(self.client.get(self.url.join("v0/tokens")?))
            .await?;
        let tokens = response
            .as_array()
            .context("Provider token catalog must be an array")?;
        for project in &config.projects {
            for (chain, binding) in &project.domains {
                validate_asset(*chain, binding, tokens)?;
            }
        }
        Ok(())
    }

    async fn quote_routes(
        &self,
        candidates: Vec<RouteCandidate>,
        mut required: u64,
    ) -> Result<(u64, Vec<QuotedRoute>, FundingWaitReason)> {
        let mut quotes = Vec::new();
        let mut reason = FundingWaitReason::InsufficientLiquidity;
        for mut candidate in candidates {
            if required == 0 {
                break;
            }
            candidate.request.amount = required.min(candidate.available).to_string();
            match self
                .affordable_quote(&mut candidate.request, candidate.available)
                .await?
            {
                QuoteAvailability::Affordable => {}
                QuoteAvailability::InsufficientLiquidity => continue,
                QuoteAvailability::ProviderDeclined => {
                    reason = FundingWaitReason::ProviderQuoteUnavailable;
                    continue;
                }
            }
            candidate.request.dry = false;
            candidate.request.correlation_id =
                self.local_simulator.then(|| candidate.route.clone());
            let quote = self.quote(&candidate.request).await?;
            if quote.input > candidate.available {
                continue;
            }
            let deposit = recipient_identity(
                candidate.chain,
                quote
                    .deposit_address
                    .as_deref()
                    .context("Live quote has no deposit address")?,
            )?;
            let expires = quote.expires.context("Live quote has no expiry")?;
            required -= quote.output;
            quotes.push(QuotedRoute {
                route: RouteQuote {
                    route: candidate.route,
                    source: candidate.domain,
                    input: quote.input,
                    min_input: quote.minimum_input,
                    output: quote.output,
                    deposit,
                    refund: candidate.request.refund_to,
                    recipient: candidate.request.recipient,
                    quote_hash: hex::encode(Sha256::digest(canonical_json(&quote.response)?)),
                    expires,
                },
                response: quote.response,
            });
        }
        Ok((required, quotes, reason))
    }

    async fn request(&self, mut request: RequestBuilder) -> Result<Value> {
        if let Some(key) = &self.api_key {
            request = request.header("X-API-Key", key);
        }
        let mut response = request
            .send()
            .await
            .map_err(|error| error.without_url())?
            .error_for_status()
            .map_err(|error| error.without_url())?;
        ensure!(
            response.content_length().unwrap_or(0) <= MAX_RESPONSE_BYTES as u64,
            "Liquidity response exceeds size limit"
        );
        let mut body = Vec::new();
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|error| error.without_url())?
        {
            ensure!(
                body.len() + chunk.len() <= MAX_RESPONSE_BYTES,
                "Liquidity response exceeds size limit"
            );
            body.extend_from_slice(&chunk);
        }
        serde_json::from_slice(&body).context("Invalid liquidity response JSON")
    }
}

#[derive(Deserialize)]
struct ApprovedRequest {
    amount: u64,
    status: String,
    local_state: u64,
    approval_position: Option<u32>,
}

fn validate_mainnet(config: &Config) -> Result<()> {
    // 1Click has no testnet deployment. RPC adapters have already verified these
    // configured identities against the connected chains.
    ensure!(
        config.ethereum.chain_id == 1
            && word(&config.ethereum.genesis_hash)? == word(ETHEREUM_MAINNET_GENESIS)?
            && config.solana.genesis_hash == SOLANA_MAINNET_GENESIS
            && config.near.genesis.chain_id == "mainnet",
        "Public 1Click routes require Ethereum, Solana and NEAR mainnet deployments"
    );
    Ok(())
}

fn validate_asset(chain: Chain, binding: &DomainConfig, tokens: &[Value]) -> Result<()> {
    let (blockchain, native_usdc) = match chain {
        Chain::Ethereum => ("eth", ETHEREUM_USDC),
        Chain::Solana => ("sol", SOLANA_USDC),
        Chain::Near => ("near", NEAR_USDC),
    };
    let native_usdc = recipient_identity(chain, native_usdc)?;
    ensure!(
        recipient_identity(chain, &binding.asset)? == native_usdc,
        "Public liquidity supports only the native mainnet USDC token"
    );
    let mut matches = tokens
        .iter()
        .filter(|token| token["assetId"] == binding.asset_id);
    let token = matches
        .next()
        .context("Configured asset is absent from the provider catalog")?;
    ensure!(
        matches.next().is_none(),
        "Ambiguous provider asset identity"
    );
    let address = token["contractAddress"]
        .as_str()
        .context("Provider asset has no native contract address")?;
    ensure!(
        token["blockchain"] == blockchain
            && token["decimals"] == 6
            && recipient_identity(chain, address)? == native_usdc,
        "Provider asset does not match the Project's six-decimal native USDC token"
    );
    Ok(())
}

// Field selection follows the official SDK at ae8b24e064785c8cba8081263e03e52fed4de089.
// Signing uses JavaScript JSON numbers/UTF-8, unlike our protocol's ASCII quote commitment.
fn signed_quote_hash(response: &Value) -> Result<String> {
    let request = &response["quoteRequest"];
    let quote = &response["quote"];
    let dry = request["dry"]
        .as_bool()
        .context("Signed quote has no dry flag")?;
    let mut signed = serde_json::Map::new();
    for (source, fields) in [
        (
            request,
            &[
                "dry",
                "swapType",
                "slippageTolerance",
                "originAsset",
                "depositType",
                "destinationAsset",
                "amount",
                "refundTo",
                "refundType",
                "recipient",
                "recipientType",
                "deadline",
            ][..],
        ),
        (
            quote,
            &[
                "amountIn",
                "amountInFormatted",
                "amountInUsd",
                "minAmountIn",
                "amountOut",
                "amountOutFormatted",
                "amountOutUsd",
                "minAmountOut",
            ][..],
        ),
        (response, &["timestamp"][..]),
    ] {
        for field in fields {
            let value = source
                .get(*field)
                .with_context(|| format!("Signed quote is missing {field}"))?;
            signed.insert((*field).into(), value.clone());
        }
    }
    for (source, fields) in [
        (
            request,
            &[
                "quoteWaitingTimeMs",
                "referral",
                "virtualChainRecipient",
                "virtualChainRefundRecipient",
                "customRecipientMsg",
            ][..],
        ),
        (
            quote,
            if dry {
                &[][..]
            } else {
                &[
                    "depositAddress",
                    "depositMemo",
                    "deadline",
                    "timeWhenInactive",
                    "timeEstimate",
                    "refundFee",
                    "withdrawFee",
                ][..]
            },
        ),
    ] {
        for field in fields {
            if let Some(value) = source.get(*field) {
                // The SDK omits null, empty strings, false and zero optional values.
                let present = match value {
                    Value::Null => false,
                    Value::Bool(value) => *value,
                    Value::String(value) => !value.is_empty(),
                    Value::Number(value) => value.as_f64().is_some_and(|number| number != 0.0),
                    _ => anyhow::bail!("Invalid signed quote field {field}"),
                };
                if present {
                    signed.insert((*field).into(), value.clone());
                }
            }
        }
    }
    let bytes = serde_json_canonicalizer::to_vec(&signed)?;
    Ok(bs58::encode(Sha256::digest(bytes)).into_string())
}

fn verify_quote_signature(response: &Value, key: &str) -> Result<()> {
    let signature = response["signature"]
        .as_str()
        .context("Public quote has no signature")?;
    let signature =
        bs58::decode(signature.strip_prefix("ed25519:").unwrap_or(signature)).into_vec()?;
    let key: [u8; 32] = bs58::decode(key.strip_prefix("ed25519:").unwrap_or(key))
        .into_vec()?
        .try_into()
        .map_err(|_| anyhow::anyhow!("Invalid provider signing key"))?;
    VerifyingKey::from_bytes(&key)?
        .verify_strict(
            signed_quote_hash(response)?.as_bytes(),
            &Signature::from_slice(&signature)?,
        )
        .context("Invalid 1Click quote signature")
}

fn validate_quote(request: &QuoteRequest, response: Value, local: bool) -> Result<Quote> {
    ensure!(
        request.swap_type == "EXACT_OUTPUT"
            && request.deposit_type == "ORIGIN_CHAIN"
            && request.recipient_type == "DESTINATION_CHAIN"
            && request.refund_type == "ORIGIN_CHAIN",
        "Only native exact-output routes are supported"
    );
    ensure!(
        request.slippage_tolerance <= 1_000
            && !request.recipient.is_empty()
            && !request.refund_to.is_empty(),
        "Invalid route terms"
    );
    let expected = serde_json::to_value(request)?;
    for (field, value) in expected.as_object().context("Invalid quote request")? {
        let actual = &response["quoteRequest"][field];
        if field == "deadline" {
            // 1Click canonicalizes ISO timestamps, e.g. Z becomes .000Z.
            // Compare the exact instant, preserving fractional-second changes.
            let returned = actual.as_str().context("Quote request has no deadline")?;
            ensure!(
                DateTime::parse_from_rfc3339(returned)?
                    == DateTime::parse_from_rfc3339(&request.deadline)?,
                "Provider changed requested deadline"
            );
        } else {
            ensure!(actual == value, "Provider changed requested {field}");
        }
    }
    ensure!(
        local || response.get("localMock").is_none(),
        "Public provider returned simulator data"
    );
    if !local {
        let terms = &response["quoteRequest"];
        ensure!(
            terms["depositMode"].is_null() || terms["depositMode"] == "SIMPLE",
            "Vault dispatch supports only memo-free deposits"
        );
        for field in [
            "virtualChainRecipient",
            "virtualChainRefundRecipient",
            "customRecipientMsg",
        ] {
            ensure!(
                terms[field].is_null() || terms[field] == "",
                "Quote added unsupported routing metadata {field}"
            );
        }
    }
    let quoted = &response["quote"];
    let input = amount(&quoted["amountIn"])?;
    let minimum_input = amount(&quoted["minAmountIn"])?;
    let output = amount(&quoted["amountOut"])?;
    ensure!(
        input >= minimum_input
            && minimum_input >= output
            && output == amount(&quoted["minAmountOut"])?
            && output == amount(&Value::String(request.amount.clone()))?,
        "Quote does not preserve exact output and fee bounds"
    );
    let deposit_address = quoted["depositAddress"].as_str().map(str::to_owned);
    let expires = quoted["deadline"]
        .as_str()
        .map(parse_deadline)
        .transpose()?;
    if !request.dry {
        ensure!(
            deposit_address
                .as_ref()
                .is_some_and(|address| !address.is_empty()),
            "Quote has no deposit address"
        );
        ensure!(
            expires.context("Quote has no deadline")? >= parse_deadline(&request.deadline)?,
            "Quote expires before requested deadline"
        );
        if local {
            let correlation = request
                .correlation_id
                .as_deref()
                .context("Simulator route identity missing")?;
            ensure!(
                response["localMock"] == true
                    && response["correlationId"] == correlation
                    && quoted["depositMemo"] == correlation,
                "Simulator changed route attribution"
            );
        } else {
            ensure!(
                quoted["depositMemo"].is_null() || quoted["depositMemo"] == "",
                "Vault dispatch cannot supply the memo required by this quote"
            );
        }
    }
    Ok(Quote {
        response,
        input,
        minimum_input,
        output,
        deposit_address,
        expires,
    })
}

struct RouteCandidate {
    chain: Chain,
    domain: String,
    route: String,
    available: u64,
    request: QuoteRequest,
}

struct QuotedRoute {
    route: RouteQuote,
    response: Value,
}

async fn route_candidates(
    indexer: &Indexer,
    project: &ProjectConfig,
    destination: Chain,
    domains: &std::collections::BTreeMap<Chain, crate::chains::near::CoordinatorDomain>,
    slippage_bps: u16,
    expires: u64,
) -> Result<Vec<RouteCandidate>> {
    let mut sources: Vec<_> = Chain::ALL
        .into_iter()
        .filter(|chain| *chain != destination && domains[chain].available > 0)
        .collect();
    sources.sort_by_key(|chain| std::cmp::Reverse(domains[chain].available));
    let mut candidates = Vec::with_capacity(sources.len());
    for source in sources {
        let route = random_identity();
        let recipient = route_receiver(indexer, project, destination, &route, false).await?;
        let refund = route_receiver(indexer, project, source, &route, true).await?;
        candidates.push(RouteCandidate {
            chain: source,
            domain: project.domains[&source].domain.clone(),
            route,
            available: domains[&source].available,
            request: QuoteRequest {
                dry: true,
                swap_type: "EXACT_OUTPUT".into(),
                slippage_tolerance: slippage_bps,
                origin_asset: project.domains[&source].asset_id.clone(),
                destination_asset: project.domains[&destination].asset_id.clone(),
                deposit_type: "ORIGIN_CHAIN".into(),
                recipient_type: "DESTINATION_CHAIN".into(),
                refund_type: "ORIGIN_CHAIN".into(),
                recipient,
                refund_to: refund,
                amount: domains[&source].available.to_string(),
                deadline: deadline(expires)?,
                correlation_id: None,
            },
        });
    }
    Ok(candidates)
}

async fn route_receiver(
    indexer: &Indexer,
    project: &ProjectConfig,
    chain: Chain,
    route: &str,
    refund: bool,
) -> Result<String> {
    let binding = &project.domains[&chain];
    Ok(match chain {
        Chain::Ethereum => {
            let block = indexer.ethereum.finalized_block().await?;
            indexer
                .ethereum
                .route(binding.vault.parse()?, word(route)?.into(), refund, &block)
                .await?
                .escrow
                .to_string()
        }
        Chain::Solana => indexer
            .solana(project)?
            .route_receiver(word(route)?)
            .to_string(),
        Chain::Near => {
            let block = indexer.near.finalized_block().await?;
            indexer
                .near
                .route_receiver(&binding.vault, route, &block)
                .await?
        }
    })
}

fn recipient_identity(chain: Chain, recipient: &str) -> Result<String> {
    let bytes = match chain {
        Chain::Ethereum => {
            let address: Address = recipient
                .parse()
                .context("Invalid Ethereum deposit address")?;
            ensure!(address != Address::ZERO, "Deposit address is zero");
            let mut word = [0; 32];
            word[12..].copy_from_slice(address.as_slice());
            word
        }
        Chain::Solana => {
            let bytes: [u8; 32] = bs58::decode(recipient)
                .into_vec()?
                .try_into()
                .map_err(|_| anyhow::anyhow!("Invalid Solana deposit address"))?;
            ensure!(bytes != [0; 32], "Deposit address is zero");
            bytes
        }
        Chain::Near => {
            recipient
                .parse::<AccountId>()
                .context("Invalid NEAR deposit account")?;
            Sha256::digest(recipient.as_bytes()).into()
        }
    };
    Ok(hex::encode(bytes))
}

fn random_identity() -> String {
    let mut bytes = [0; 32];
    OsRng.fill_bytes(&mut bytes);
    hex::encode(bytes)
}

fn amount(value: &Value) -> Result<u64> {
    let text = value
        .as_str()
        .context("Quoted amounts must be decimal strings")?;
    ensure!(
        !text.is_empty() && text.bytes().all(|byte| byte.is_ascii_digit()),
        "Invalid quoted amount"
    );
    let amount: u64 = text
        .parse()
        .context("Quoted amount exceeds protocol limit")?;
    ensure!(amount > 0, "Quoted amount must be positive");
    Ok(amount)
}

fn deadline(seconds: u64) -> Result<String> {
    let seconds = i64::try_from(seconds)?;
    Ok(DateTime::<Utc>::from_timestamp(seconds, 0)
        .context("Invalid quote expiry")?
        .to_rfc3339_opts(SecondsFormat::Secs, true))
}

fn parse_deadline(value: &str) -> Result<u64> {
    Ok(u64::try_from(
        DateTime::parse_from_rfc3339(value)?.timestamp(),
    )?)
}

fn now() -> Result<u64> {
    Ok(u64::try_from(Utc::now().timestamp())?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[actix_web::test]
    async fn two_sources_cover_principal_only_after_actual_affordable_quotes() {
        use actix_web::{App, HttpResponse, HttpServer, web};

        let server = HttpServer::new(|| App::new().route("/v0/quote", web::post().to(
            |request: web::Json<QuoteRequest>| async move {
                match request.origin_asset.as_str() {
                    "dry-decline" if request.dry => return HttpResponse::BadRequest().json(json!({"message":"Provider-specific explanation"})),
                    "live-decline" if !request.dry => return HttpResponse::BadRequest().finish(),
                    "unavailable" => return HttpResponse::ServiceUnavailable().finish(),
                    "unauthorized" => return HttpResponse::Unauthorized().finish(),
                    _ => {}
                }
                let output: u64 = request.amount.parse().unwrap();
                let input = output + 10 + output / 10;
                let mut quote = json!({"amountIn": input.to_string(), "minAmountIn": input.to_string(),
                    "amountOut": output.to_string(), "minAmountOut": output.to_string()});
                if request.origin_asset == "bad-terms" {
                    quote["amountOut"] = (output + 1).to_string().into();
                }
                if !request.dry {
                    quote["depositAddress"] = if request.origin_asset == "ethereum-usdc" {
                        "0x1111111111111111111111111111111111111111".into()
                    } else { "deposit.near".into() };
                    quote["depositMemo"] = json!(request.correlation_id);
                    quote["deadline"] = request.deadline.clone().into();
                }
                HttpResponse::Ok().json(json!({"quoteRequest": *request, "quote": quote,
                    "localMock": true, "correlationId": request.correlation_id}))
            }
        )))
        .workers(1).bind(("127.0.0.1", 0)).unwrap();
        let url = format!("http://{}", server.addrs()[0]);
        let server = server.run();
        let handle = server.handle();
        actix_web::rt::spawn(server);
        let mut liquidity = Liquidity::new(&LiquidityConfig {
            url,
            local_simulator: true,
            api_key_file: None,
        })
        .unwrap();
        let candidate = |chain: Chain, available, asset: &str| RouteCandidate {
            chain,
            domain: format!("{:064x}", if chain == Chain::Ethereum { 1 } else { 2 }),
            route: random_identity(),
            available,
            request: QuoteRequest {
                dry: true,
                swap_type: "EXACT_OUTPUT".into(),
                slippage_tolerance: 20,
                origin_asset: asset.into(),
                destination_asset: "solana-usdc".into(),
                deposit_type: "ORIGIN_CHAIN".into(),
                recipient_type: "DESTINATION_CHAIN".into(),
                refund_type: "ORIGIN_CHAIN".into(),
                recipient: "destination".into(),
                refund_to: format!("refund-{chain}"),
                amount: available.to_string(),
                deadline: "2030-01-01T00:00:00Z".into(),
                correlation_id: None,
            },
        };
        let candidates = vec![
            candidate(Chain::Ethereum, 100, "ethereum-usdc"),
            candidate(Chain::Near, 60, "near-usdc"),
        ];
        let (shortfall, quotes, _) = liquidity.quote_routes(candidates, 100).await.unwrap();

        assert_eq!(shortfall, 0);
        assert_eq!(quotes.len(), 2);
        assert_eq!(quotes[0].route.output, 82);
        assert_eq!(quotes[1].route.output, 18);
        let routes: Vec<_> = quotes.iter().map(|quote| quote.route.clone()).collect();
        let costs = FundingCosts::from_routes(100, 0, &routes, 500).unwrap();
        assert_eq!(costs.principal, 100);
        assert_eq!(costs.expected_transport_cost, 29);
        assert_eq!(costs.maximum_transport_cost, 29);
        assert_eq!(costs.warnings.len(), 2);
        assert!(
            FundingCosts::from_routes(100, 0, &routes, 10_000)
                .unwrap()
                .warnings
                .is_empty()
        );
        for (quote, available) in quotes.iter().zip([100, 60]) {
            assert!(quote.route.input <= available);
            assert_eq!(
                quote.response["quoteRequest"]["amount"],
                quote.route.output.to_string()
            );
            assert_eq!(quote.response["quoteRequest"]["dry"], false);
            assert_eq!(
                quote.route.quote_hash,
                hex::encode(Sha256::digest(canonical_json(&quote.response).unwrap()))
            );
        }

        let (shortfall, routes, _) = liquidity
            .quote_routes(
                vec![
                    candidate(Chain::Ethereum, 200, "dry-decline"),
                    candidate(Chain::Near, 200, "near-usdc"),
                ],
                100,
            )
            .await
            .unwrap();
        assert_eq!(shortfall, 0);
        assert_eq!(routes.len(), 1);
        assert_eq!(routes[0].route.output, 100);

        let (shortfall, routes, reason) = liquidity
            .quote_routes(vec![candidate(Chain::Ethereum, 200, "dry-decline")], 100)
            .await
            .unwrap();
        assert_eq!(shortfall, 100);
        assert!(routes.is_empty());
        assert_eq!(reason, FundingWaitReason::ProviderQuoteUnavailable);
        let (shortfall, routes, reason) = liquidity
            .quote_routes(vec![candidate(Chain::Near, 5, "near-usdc")], 100)
            .await
            .unwrap();
        assert_eq!(shortfall, 100);
        assert!(routes.is_empty());
        assert_eq!(reason, FundingWaitReason::InsufficientLiquidity);

        for asset in ["unavailable", "unauthorized", "live-decline", "bad-terms"] {
            assert!(
                liquidity
                    .quote_routes(vec![candidate(Chain::Near, 200, asset)], 100)
                    .await
                    .is_err(),
                "{asset} must remain an explicit preparation error"
            );
        }
        liquidity.local_simulator = false;
        assert!(
            liquidity
                .quote_routes(vec![candidate(Chain::Near, 200, "unsigned")], 100)
                .await
                .is_err(),
            "an unsigned quote must not become an availability shortfall"
        );
        handle.stop(true).await;
    }

    #[test]
    fn quote_terms_cannot_redirect_or_underfund_the_route() {
        let request = QuoteRequest {
            dry: false,
            swap_type: "EXACT_OUTPUT".into(),
            slippage_tolerance: 20,
            origin_asset: "source-usdc".into(),
            destination_asset: "destination-usdc".into(),
            deposit_type: "ORIGIN_CHAIN".into(),
            recipient_type: "DESTINATION_CHAIN".into(),
            refund_type: "ORIGIN_CHAIN".into(),
            recipient: "receiver.near".into(),
            refund_to: "refund.near".into(),
            amount: "1000000".into(),
            deadline: "2030-01-01T00:00:00Z".into(),
            correlation_id: None,
        };
        let response = json!({"quoteRequest": request, "quote": {
            "amountIn": "1010000", "minAmountIn": "1005000", "amountOut": "1000000", "minAmountOut": "1000000",
            "depositAddress": "deposit.near", "deadline": "2030-01-01T00:00:00Z",
        }});
        assert_eq!(
            validate_quote(&request, response.clone(), false)
                .unwrap()
                .output,
            1_000_000
        );
        for (field, value) in [
            ("minAmountOut", json!("999999")),
            ("amountIn", json!("1000000")),
            ("depositMemo", json!("cannot-send-memo")),
            ("amountIn", json!("18446744073709551616")),
        ] {
            let mut changed = response.clone();
            changed["quote"][field] = value;
            assert!(validate_quote(&request, changed, false).is_err());
        }
        let mut changed = response;
        changed["quoteRequest"]["recipient"] = "another.near".into();
        assert!(validate_quote(&request, changed, false).is_err());

        let binding = DomainConfig {
            domain: "01".repeat(32),
            vault: "vault.near".into(),
            asset: NEAR_USDC.into(),
            asset_id: format!("nep141:{NEAR_USDC}"),
        };
        let token = json!({"assetId":binding.asset_id,"blockchain":"near","decimals":6,"contractAddress":binding.asset});
        validate_asset(Chain::Near, &binding, std::slice::from_ref(&token)).unwrap();
        for (field, value) in [
            ("assetId", json!("another-asset")),
            ("blockchain", json!("eth")),
            ("decimals", json!(18)),
            ("contractAddress", json!("different.near")),
        ] {
            let mut wrong = token.clone();
            wrong[field] = value;
            assert!(validate_asset(Chain::Near, &binding, &[wrong]).is_err());
        }
        for (chain, blockchain, usdc, usdt) in [
            (
                Chain::Ethereum,
                "eth",
                ETHEREUM_USDC,
                "0xdac17f958d2ee523a2206206994597c13d831ec7",
            ),
            (
                Chain::Solana,
                "sol",
                SOLANA_USDC,
                "Es9vMFrzaCERmJfrF4H2FYD4KCoNkY11McCe8BenwNYB",
            ),
            (Chain::Near, "near", NEAR_USDC, "usdt.tether-token.near"),
        ] {
            let mut binding = DomainConfig {
                domain: "01".repeat(32),
                vault: "unused".into(),
                asset: usdc.into(),
                asset_id: "provider-native-asset".into(),
            };
            let catalog = json!({"assetId":binding.asset_id,"blockchain":blockchain,
                "decimals":6,"symbol":"USDC","contractAddress":if chain == Chain::Ethereum { usdc.to_ascii_lowercase() } else { usdc.into() }});
            validate_asset(chain, &binding, &[catalog]).unwrap();

            binding.asset = usdt.into();
            let catalog = json!({"assetId":binding.asset_id,"blockchain":blockchain,
                "decimals":6,"symbol":"USDC","contractAddress":usdt});
            assert!(
                validate_asset(chain, &binding, &[catalog]).is_err(),
                "a matching six-decimal catalog entry and misleading symbol cannot admit USDT"
            );
        }
        let mut config: Config = serde_json::from_value(json!({
            "ethereum":{"rpc_url":"https://ethereum.invalid","chain_id":1,"genesis_hash":ETHEREUM_MAINNET_GENESIS},
            "solana":{"rpc_url":"https://solana.invalid","genesis_hash":SOLANA_MAINNET_GENESIS},
            "near":{"rpc_url":"https://near.invalid","genesis":{"chain_id":"mainnet","genesis_time":"verified-by-rpc-adapter","genesis_height":0,"validators":[]}},
            "projects":[], "liquidity":{"url":"https://1click.chaindefuser.com","local_simulator":false,"api_key_file":null}
        })).unwrap();
        validate_mainnet(&config).unwrap();
        config.ethereum.chain_id = 11155111;
        assert!(validate_mainnet(&config).is_err());
        config.ethereum.chain_id = 1;
        config.solana.genesis_hash = "testnet".into();
        assert!(validate_mainnet(&config).is_err());
        config.solana.genesis_hash = SOLANA_MAINNET_GENESIS.into();
        config.near.genesis.chain_id = "testnet".into();
        assert!(validate_mainnet(&config).is_err());
    }

    #[test]
    fn production_quote_preserves_terms_after_provider_timestamp_normalization() {
        let fixture: Value = serde_json::from_str(include_str!(
            "../tests/fixtures/oneclick-production-quote.json"
        ))
        .unwrap();
        let request: QuoteRequest = serde_json::from_value(fixture["request"].clone()).unwrap();
        let response = &fixture["response"];
        verify_quote_signature(response, ONE_CLICK_KEY).unwrap();
        assert_ne!(response["quoteRequest"]["deadline"], request.deadline);
        assert_eq!(
            validate_quote(&request, response.clone(), false)
                .unwrap()
                .output,
            100_000_000
        );

        for deadline in ["2026-11-11T19:07:32Z", "2026-11-11T19:07:33.001Z"] {
            let mut changed = response.clone();
            changed["quoteRequest"]["deadline"] = deadline.into();
            assert!(validate_quote(&request, changed, false).is_err());
        }
        for (field, value) in [
            ("depositMode", "MEMO"),
            ("virtualChainRecipient", "redirected"),
            ("virtualChainRefundRecipient", "redirected"),
            ("customRecipientMsg", "unexpected-callback"),
        ] {
            let mut changed = response.clone();
            changed["quoteRequest"][field] = value.into();
            assert!(validate_quote(&request, changed, false).is_err());
        }
    }

    #[test]
    fn official_provider_fixtures_authenticate_execution_terms() {
        let fixture: Value =
            serde_json::from_str(include_str!("../tests/fixtures/oneclick-signatures.json"))
                .unwrap();
        let key = fixture["public_key"].as_str().unwrap();
        for quote in fixture["quotes"].as_array().unwrap() {
            verify_quote_signature(quote, key).unwrap();
            assert!(verify_quote_signature(quote, ONE_CLICK_KEY).is_err());
            for (section, field) in [
                ("quoteRequest", "recipient"),
                ("quoteRequest", "refundTo"),
                ("quote", "amountOut"),
                ("quote", "amountIn"),
            ] {
                let mut changed = quote.clone();
                changed[section][field] = "substituted".into();
                assert!(verify_quote_signature(&changed, key).is_err());
            }
            if quote["quoteRequest"]["dry"] == false {
                for field in ["depositAddress", "deadline"] {
                    let mut changed = quote.clone();
                    changed["quote"][field] = "substituted".into();
                    assert!(verify_quote_signature(&changed, key).is_err());
                }
                // JavaScript stringifies 812 and 812.0 identically; signatures must agree.
                let mut float = quote.clone();
                float["quote"]["timeEstimate"] = json!(812.0);
                verify_quote_signature(&float, key).unwrap();
            }
        }
    }
}
