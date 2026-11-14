//! Same-origin browser API. Wallet signatures authorize users; this layer never
//! signs owner, authority or agent transactions.
use std::time::Duration;

mod auth;
mod references;
pub use auth::BrowserConfig;
use auth::authenticated;
#[cfg(test)]
mod tests;
mod views;

use actix_web::{
    HttpRequest, HttpResponse, ResponseError,
    http::{StatusCode, header},
    middleware::{DefaultHeaders, from_fn},
    web,
};
use anyhow::Result;
use serde::Deserialize;
use serde_json::{Value, json};

use crate::{
    accounts::ProfileError,
    actions::{Actions, ApplyRequest, CoordinatorAction, LocalAction},
    application::Application,
    config::{Chain, ProjectConfig},
    rpc::{Rpc, RpcError},
};

#[derive(Debug)]
struct ApiError {
    status: StatusCode,
    code: &'static str,
    message: &'static str,
}
impl ApiError {
    fn new(status: StatusCode, code: &'static str, message: &'static str) -> Self {
        Self {
            status,
            code,
            message,
        }
    }
    fn unauthorized() -> Self {
        Self::new(
            StatusCode::UNAUTHORIZED,
            "authentication_required",
            "Connect and verify a wallet to continue.",
        )
    }
    fn invalid(message: &'static str) -> Self {
        Self::new(StatusCode::UNPROCESSABLE_ENTITY, "invalid_request", message)
    }
    fn service(error: anyhow::Error) -> Self {
        tracing::warn!(error = %error, "Browser service request failed");
        Self::new(
            StatusCode::SERVICE_UNAVAILABLE,
            "service_unavailable",
            "The service could not verify current state. Please try again shortly.",
        )
    }
    fn unavailable(error: &anyhow::Error) -> bool {
        error.downcast_ref::<sqlx::Error>().is_some()
            || error.downcast_ref::<reqwest::Error>().is_some()
            || error.downcast_ref::<RpcError>().is_some()
    }

    fn session(error: anyhow::Error) -> Self {
        if Self::unavailable(&error) {
            Self::service(error)
        } else {
            Self::unauthorized()
        }
    }

    fn proof(error: anyhow::Error) -> Self {
        if Self::unavailable(&error) {
            return Self::service(error);
        }
        Self::new(
            StatusCode::UNAUTHORIZED,
            "invalid_proof",
            "The signature is invalid or the sign-in request has expired. Connect your wallet again.",
        )
    }

    fn action(error: anyhow::Error) -> Self {
        if Self::unavailable(&error) {
            return Self::service(error);
        }
        tracing::info!(error = %error, "Wallet action preparation rejected");
        Self::new(
            StatusCode::UNPROCESSABLE_ENTITY,
            "action_rejected",
            "This action cannot be prepared for the selected wallet and current contract state. Refresh the project and check its permissions and available funds.",
        )
    }

    fn profile(error: anyhow::Error) -> Self {
        match error.downcast_ref::<ProfileError>() {
            Some(ProfileError::UsernameTaken) => Self::new(
                StatusCode::CONFLICT,
                "username_taken",
                ProfileError::UsernameTaken.message(),
            ),
            Some(error) => Self::new(
                StatusCode::UNPROCESSABLE_ENTITY,
                "invalid_profile",
                error.message(),
            ),
            None => Self::service(error),
        }
    }
}
impl std::fmt::Display for ApiError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.message)
    }
}
impl std::error::Error for ApiError {}
impl ResponseError for ApiError {
    fn status_code(&self) -> StatusCode {
        self.status
    }
    fn error_response(&self) -> HttpResponse {
        HttpResponse::build(self.status)
            .insert_header((header::CACHE_CONTROL, "no-store"))
            .json(json!({"error":{"code":self.code,"message":self.message}}))
    }
}
type ApiResult = std::result::Result<HttpResponse, ApiError>;

pub fn configure(config: &mut web::ServiceConfig) {
    config.service(
        web::scope("/api")
            .app_data(
                web::JsonConfig::default()
                    .limit(128 * 1024)
                    .error_handler(|error, _| {
                        let status = match error {
                            actix_web::error::JsonPayloadError::OverflowKnownLength { .. }
                            | actix_web::error::JsonPayloadError::Overflow { .. } => {
                                StatusCode::PAYLOAD_TOO_LARGE
                            }
                            _ => StatusCode::BAD_REQUEST,
                        };
                        ApiError::new(
                            status,
                            "invalid_json",
                            "The request must contain valid JSON with the required fields.",
                        )
                        .into()
                    }),
            )
            .wrap(
                DefaultHeaders::new()
                    .add((header::CACHE_CONTROL, "no-store"))
                    .add((header::X_CONTENT_TYPE_OPTIONS, "nosniff")),
            )
            .wrap(from_fn(auth::browser_guard))
            .route("/health/ready", web::get().to(readiness))
            .route("/config", web::get().to(public_config))
            .route("/auth/challenge", web::post().to(auth::login_challenge))
            .route("/auth/verify", web::post().to(auth::login))
            .route("/auth/logout", web::post().to(auth::logout))
            .route("/account", web::get().to(auth::account))
            .route("/account", web::patch().to(auth::update_profile))
            .route(
                "/profiles/{username}",
                web::get().to(auth::profile_by_username),
            )
            .route(
                "/account/wallets/challenge",
                web::post().to(auth::link_challenge),
            )
            .route("/account/wallets/verify", web::post().to(auth::link_wallet))
            .route("/projects", web::get().to(projects))
            .route("/projects/{key}", web::get().to(project))
            .route(
                "/projects/{key}/references",
                web::post().to(references::create),
            )
            .route(
                "/projects/{key}/references/{digest}",
                web::get().to(references::read),
            )
            .route(
                "/projects/{key}/actions/local",
                web::post().to(prepare_local),
            )
            .route(
                "/projects/{key}/actions/coordinator",
                web::post().to(prepare_coordinator),
            )
            .route(
                "/projects/{key}/actions/apply",
                web::post().to(prepare_apply),
            )
            .route(
                "/projects/{key}/routes/{route}/status",
                web::get().to(transport_status),
            )
            .route("/rpc/{chain}", web::post().to(wallet_rpc))
            .default_service(web::to(|| async {
                ApiError::new(
                    StatusCode::NOT_FOUND,
                    "not_found",
                    "This API resource does not exist.",
                )
                .error_response()
            })),
    );
}

fn deployment<'a>(
    app: &'a Application,
    key: &str,
) -> std::result::Result<&'a ProjectConfig, ApiError> {
    app.indexer
        .config
        .projects
        .iter()
        .find(|project| app.indexer.config.project_key(project) == key)
        .ok_or_else(|| {
            ApiError::new(
                StatusCode::NOT_FOUND,
                "project_not_found",
                "This project is not registered in this deployment.",
            )
        })
}

/// The HTTP listener alone is not ready for the workspace: every configured
/// Project needs its initial coordinator and three local vault projections.
/// Live RPC availability is tracked separately by the synchronization worker.
async fn readiness(app: web::Data<Application>) -> ApiResult {
    let projects: Vec<_> = app
        .indexer
        .config
        .projects
        .iter()
        .map(|project| app.indexer.config.project_key(project))
        .collect();
    let ready: bool = tokio::time::timeout(
        Duration::from_secs(2),
        sqlx::query_scalar(
            "SELECT count(*) = cardinality($1::text[]) FROM project_views AS p \
             WHERE p.project = ANY($1) AND \
             (SELECT count(*) FROM vault_views AS v WHERE v.project = p.project) = 3",
        )
        .bind(&projects)
        .fetch_one(&app.indexer.store.pool),
    )
    .await
    .map_err(|_| {
        ApiError::new(
            StatusCode::SERVICE_UNAVAILABLE,
            "readiness_timeout",
            "The database did not respond to the readiness check in time.",
        )
    })?
    .map_err(|error| ApiError::service(error.into()))?;
    if !ready {
        return Err(ApiError::new(
            StatusCode::SERVICE_UNAVAILABLE,
            "indexing_pending",
            "The initial project and vault snapshots are still being indexed.",
        ));
    }
    Ok(HttpResponse::NoContent().finish())
}

async fn public_config(
    app: web::Data<Application>,
    browser: web::Data<BrowserConfig>,
) -> ApiResult {
    let config = &app.indexer.config;
    Ok(HttpResponse::Ok().json(json!({
        "origin": browser.origin,
        "local_simulator": config.liquidity.local_simulator,
        "wallet_networks": {"ethereum": app.accounts.network(Chain::Ethereum), "solana": app.accounts.network(Chain::Solana), "near": app.accounts.network(Chain::Near)},
        "ethereum": {"chain_id":config.ethereum.chain_id.to_string(),"genesis_hash":config.ethereum.genesis_hash},
        "solana": {"genesis_hash":config.solana.genesis_hash},
        "near": {"network_id":config.near.genesis.chain_id,"genesis":config.near.genesis},
    })))
}
async fn projects(app: web::Data<Application>, request: HttpRequest) -> ApiResult {
    let session = authenticated(&app, &request).await?;
    let wallets = app
        .accounts
        .wallets(&session)
        .await
        .map_err(ApiError::service)?;
    let mut projects = Vec::new();
    for project in &app.indexer.config.projects {
        projects.push(
            views::summary(&app, project, &wallets)
                .await
                .map_err(ApiError::service)?,
        );
    }
    Ok(HttpResponse::Ok().json(projects))
}
async fn project(
    app: web::Data<Application>,
    request: HttpRequest,
    key: web::Path<String>,
) -> ApiResult {
    let session = authenticated(&app, &request).await?;
    let project = deployment(&app, &key)?;
    let wallets = app
        .accounts
        .wallets(&session)
        .await
        .map_err(ApiError::service)?;
    let detail = views::detail(&app, project, &wallets).await?;
    Ok(HttpResponse::Ok().json(detail))
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct LocalInput {
    actor: String,
    chain: Chain,
    action: LocalAction,
    storage_deposit: Option<String>,
    token_account: Option<String>,
}
async fn prepare_local(
    app: web::Data<Application>,
    request: HttpRequest,
    key: web::Path<String>,
    input: web::Json<LocalInput>,
) -> ApiResult {
    let session = authenticated(&app, &request).await?;
    deployment(&app, &key)?;
    let input = input.into_inner();
    let actions = Actions::new(&app.accounts, &app.indexer);
    let intent = match input.chain {
        Chain::Ethereum => {
            if input.storage_deposit.is_some() || input.token_account.is_some() {
                return Err(ApiError::invalid(
                    "Ethereum does not use NEAR storage or Solana token accounts.",
                ));
            }
            actions
                .prepare_ethereum(&session, &key, &input.actor, input.action)
                .await
        }
        Chain::Solana => {
            if input.storage_deposit.is_some() {
                return Err(ApiError::invalid(
                    "Solana does not use a NEAR storage deposit.",
                ));
            }
            actions
                .prepare_solana(
                    &session,
                    &key,
                    &input.actor,
                    input.action,
                    input.token_account.as_deref(),
                )
                .await
        }
        Chain::Near => {
            if input.token_account.is_some() {
                return Err(ApiError::invalid(
                    "NEAR does not use a Solana token account.",
                ));
            }
            let deposit = decimal_deposit(
                input
                    .storage_deposit
                    .as_deref()
                    .ok_or_else(|| ApiError::invalid("NEAR storage_deposit is required."))?,
            )?;
            actions
                .prepare_near(&session, &key, &input.actor, input.action, deposit)
                .await
        }
    }
    .map_err(ApiError::action)?;
    Ok(HttpResponse::Ok().json(intent))
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CoordinatorInput {
    actor: String,
    action: CoordinatorAction,
    storage_deposit: String,
}
async fn prepare_coordinator(
    app: web::Data<Application>,
    request: HttpRequest,
    key: web::Path<String>,
    input: web::Json<CoordinatorInput>,
) -> ApiResult {
    let session = authenticated(&app, &request).await?;
    deployment(&app, &key)?;
    let input = input.into_inner();
    let intent = Actions::new(&app.accounts, &app.indexer)
        .prepare_coordinator(
            &session,
            &key,
            &input.actor,
            input.action,
            decimal_deposit(&input.storage_deposit)?,
        )
        .await
        .map_err(ApiError::action)?;
    Ok(HttpResponse::Ok().json(intent))
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ApplyInput {
    actor: String,
    request: ApplyRequest,
}
async fn prepare_apply(
    app: web::Data<Application>,
    request: HttpRequest,
    key: web::Path<String>,
    input: web::Json<ApplyInput>,
) -> ApiResult {
    let session = authenticated(&app, &request).await?;
    deployment(&app, &key)?;
    let input = input.into_inner();
    let prepared = Actions::new(&app.accounts, &app.indexer)
        .prepare_apply(&session, &key, &input.actor, input.request, &app.liquidity)
        .await
        .map_err(ApiError::action)?;
    Ok(HttpResponse::Ok().json(prepared))
}
fn decimal_deposit(value: &str) -> std::result::Result<u128, ApiError> {
    if value.is_empty() || !value.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(ApiError::invalid(
            "Storage deposit must be an unsigned decimal string.",
        ));
    }
    value
        .parse()
        .map_err(|_| ApiError::invalid("Storage deposit exceeds the supported amount."))
}
async fn transport_status(
    app: web::Data<Application>,
    request: HttpRequest,
    path: web::Path<(String, String)>,
) -> ApiResult {
    authenticated(&app, &request).await?;
    let (key, route) = path.into_inner();
    deployment(&app, &key)?;
    let route = hex::encode(
        crate::config::word(&route).map_err(|_| ApiError::invalid("Invalid route identifier."))?,
    );
    let quote: Option<Value> =
        sqlx::query_scalar("SELECT response FROM funding_quotes WHERE project=$1 AND route=$2")
            .bind(key)
            .bind(route)
            .fetch_optional(&app.indexer.store.pool)
            .await
            .map_err(|e| ApiError::service(e.into()))?;
    let quote = quote.ok_or_else(|| {
        ApiError::new(
            StatusCode::NOT_FOUND,
            "route_not_found",
            "No provider quote is recorded for this route.",
        )
    })?;
    Ok(HttpResponse::Ok().json(
        app.liquidity
            .status(&quote)
            .await
            .map_err(ApiError::service)?,
    ))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RpcInput {
    jsonrpc: String,
    id: Value,
    method: String,
    params: Value,
}
async fn wallet_rpc(
    app: web::Data<Application>,
    request: HttpRequest,
    chain: web::Path<Chain>,
    input: web::Json<RpcInput>,
) -> ApiResult {
    authenticated(&app, &request).await?;
    validate_wallet_rpc(*chain, &input)?;
    let config = &app.indexer.config;
    let url = match *chain {
        Chain::Ethereum => &config.ethereum.rpc_url,
        Chain::Solana => &config.solana.rpc_url,
        Chain::Near => &config.near.rpc_url,
    };
    let result: Result<Value> = Rpc::new(url)
        .map_err(ApiError::service)?
        .call(&input.method, &input.params)
        .await;
    match result {
        Ok(result) => {
            Ok(HttpResponse::Ok().json(json!({"jsonrpc":"2.0","id":input.id,"result":result})))
        }
        Err(error) => {
            if let Some(error) = error.downcast_ref::<RpcError>() {
                let description = format!("{} {:?} {:?}", error.message, error.cause, error.data)
                    .to_ascii_uppercase();
                let missing = description.contains("UNKNOWN_TRANSACTION")
                    || description.contains("UNKNOWNTRANSACTION")
                    || description.contains("TRANSACTION NOT FOUND");
                let message = if missing {
                    "UNKNOWN_TRANSACTION"
                } else {
                    "The chain rejected this RPC request."
                };
                return Ok(HttpResponse::Ok().json(json!({
                    "jsonrpc": "2.0",
                    "id": input.id,
                    "error": {"code":error.code,"message":message,"data":{"kind":if missing {"not_found"}else{"rejected"}}},
                })));
            }
            Err(ApiError::service(error))
        }
    }
}
fn validate_wallet_rpc(chain: Chain, input: &RpcInput) -> std::result::Result<(), ApiError> {
    let allowed: &[&str] = match chain {
        Chain::Ethereum => &[
            "eth_chainId",
            "eth_getBlockByNumber",
            "eth_getTransactionReceipt",
            "eth_getTransactionByHash",
            "eth_getTransactionCount",
            "eth_estimateGas",
            "eth_gasPrice",
            "eth_sendRawTransaction",
        ],
        Chain::Solana => &[
            "getGenesisHash",
            "getLatestBlockhash",
            "getSignatureStatuses",
            "getTokenAccountsByOwner",
            "getBlockHeight",
            "sendTransaction",
        ],
        Chain::Near => &[
            "genesis_config",
            "query",
            "block",
            "broadcast_tx_async",
            "tx",
        ],
    };
    if input.jsonrpc != "2.0"
        || !allowed.contains(&input.method.as_str())
        || !(input.id.is_string() || input.id.is_u64())
    {
        return Err(ApiError::invalid(
            "This wallet RPC method or request identity is not supported.",
        ));
    }
    if chain == Chain::Solana && input.method == "getTokenAccountsByOwner" {
        let params = input
            .params
            .as_array()
            .ok_or_else(|| ApiError::invalid("Invalid token account query."))?;
        if params.len() != 3
            || params[1].get("mint").and_then(Value::as_str).is_none()
            || params[1].get("programId").is_some()
            || params[2]["encoding"] != "jsonParsed"
            || params[2]["commitment"] != "finalized"
        {
            return Err(ApiError::invalid(
                "Token account lookup requires a mint and finalized jsonParsed encoding.",
            ));
        }
    }
    if chain == Chain::Near && input.method == "query" {
        let request_type = input.params["request_type"].as_str();
        if !matches!(
            request_type,
            Some("view_access_key" | "view_access_key_list")
        ) || input.params["finality"] != "final"
        {
            return Err(ApiError::invalid(
                "Only finalized NEAR access-key lookup is available through the wallet RPC.",
            ));
        }
    }
    if chain == Chain::Ethereum
        && input.method == "eth_getBlockByNumber"
        && input.params.get(1) != Some(&Value::Bool(false))
    {
        return Err(ApiError::invalid(
            "Wallet block lookup does not include full transactions.",
        ));
    }
    Ok(())
}
