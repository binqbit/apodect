//! Browser identity and same-origin cookie boundary.
use actix_web::{
    Error, HttpRequest, HttpResponse, ResponseError,
    body::{EitherBody, MessageBody},
    cookie::{Cookie, SameSite, time::Duration},
    dev::{ServiceRequest, ServiceResponse},
    http::{Method, StatusCode, header},
    middleware::Next,
    web,
};
use anyhow::{Context, Result};
use serde::Deserialize;
use serde_json::{Value, json};
use uuid::Uuid;

use super::{ApiError, ApiResult};
use crate::{application::Application, config::Chain};

pub(super) const SESSION_COOKIE: &str = "apodect_session";

pub struct BrowserConfig {
    pub(super) origin: String,
    pub(super) secure: bool,
}

impl BrowserConfig {
    pub fn from_env() -> Result<Self> {
        let url =
            reqwest::Url::parse(&std::env::var("APP_ORIGIN").context("APP_ORIGIN is required")?)?;
        Ok(Self {
            origin: url.origin().ascii_serialization(),
            secure: url.scheme() == "https",
        })
    }
}

pub(super) async fn browser_guard(
    request: ServiceRequest,
    next: Next<impl MessageBody>,
) -> std::result::Result<ServiceResponse<EitherBody<impl MessageBody>>, Error> {
    if !matches!(
        *request.method(),
        Method::GET | Method::HEAD | Method::OPTIONS
    ) {
        let expected = request
            .app_data::<web::Data<BrowserConfig>>()
            .map(|config| config.origin.as_str());
        let origin = request
            .headers()
            .get(header::ORIGIN)
            .and_then(|value| value.to_str().ok());
        if expected.is_none() || origin != expected {
            let error = ApiError::new(
                StatusCode::FORBIDDEN,
                "origin_rejected",
                "This request must originate from the application website.",
            );
            return Ok(request
                .into_response(error.error_response())
                .map_into_right_body());
        }
    }
    Ok(next.call(request).await?.map_into_left_body())
}

fn token(request: &HttpRequest) -> std::result::Result<String, ApiError> {
    let value = request
        .cookie(SESSION_COOKIE)
        .ok_or_else(ApiError::unauthorized)?
        .value()
        .to_owned();
    if value.len() != 64 || !value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(ApiError::unauthorized());
    }
    Ok(value)
}
pub(super) async fn authenticated(
    app: &Application,
    request: &HttpRequest,
) -> std::result::Result<String, ApiError> {
    let token = token(request)?;
    app.accounts
        .session(&token)
        .await
        .map_err(ApiError::session)?;
    Ok(token)
}
fn session_cookie(token: String, config: &BrowserConfig, lifetime: i64) -> Cookie<'static> {
    Cookie::build(SESSION_COOKIE, token)
        .path("/api")
        .http_only(true)
        .secure(config.secure)
        .same_site(SameSite::Strict)
        .max_age(Duration::seconds(lifetime))
        .finish()
}

async fn account_json(app: &Application, session: &str) -> Result<Value> {
    let identity = app.accounts.session(session).await?;
    let wallets = app.accounts.wallets(session).await?;
    Ok(json!({ "session": identity, "wallets": wallets }))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ChallengeInput {
    chain: Chain,
    address: String,
    callback_url: Option<String>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ProofInput {
    challenge: Uuid,
    signature: String,
    public_key: Option<String>,
}
pub(super) async fn login_challenge(
    app: web::Data<Application>,
    input: web::Json<ChallengeInput>,
) -> ApiResult {
    let challenge = app
        .accounts
        .begin_login_with_callback(input.chain, &input.address, input.callback_url.as_deref())
        .await
        .map_err(ApiError::action)?;
    Ok(HttpResponse::Ok().json(challenge))
}
pub(super) async fn login(
    app: web::Data<Application>,
    browser: web::Data<BrowserConfig>,
    input: web::Json<ProofInput>,
) -> ApiResult {
    let login = app
        .accounts
        .complete_login(
            input.challenge,
            &input.signature,
            input.public_key.as_deref(),
        )
        .await
        .map_err(ApiError::proof)?;
    let data = account_json(&app, &login.session_token)
        .await
        .map_err(ApiError::service)?;
    Ok(HttpResponse::Ok()
        .cookie(session_cookie(
            login.session_token,
            &browser,
            (login.expires_at - chrono::Utc::now()).num_seconds().max(0),
        ))
        .json(data))
}
pub(super) async fn logout(
    app: web::Data<Application>,
    browser: web::Data<BrowserConfig>,
    request: HttpRequest,
) -> ApiResult {
    if let Ok(session) = token(&request) {
        app.accounts
            .revoke_session(&session)
            .await
            .map_err(ApiError::service)?;
    }
    Ok(HttpResponse::NoContent()
        .cookie(session_cookie(String::new(), &browser, 0))
        .finish())
}
pub(super) async fn account(app: web::Data<Application>, request: HttpRequest) -> ApiResult {
    let session = authenticated(&app, &request).await?;
    Ok(HttpResponse::Ok().json(
        account_json(&app, &session)
            .await
            .map_err(ApiError::service)?,
    ))
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct RenameInput {
    name: Option<String>,
}
pub(super) async fn rename(
    app: web::Data<Application>,
    request: HttpRequest,
    input: web::Json<RenameInput>,
) -> ApiResult {
    let session = authenticated(&app, &request).await?;
    Ok(HttpResponse::Ok().json(
        app.accounts
            .update_display_name(&session, input.name.as_deref())
            .await
            .map_err(ApiError::action)?,
    ))
}
pub(super) async fn link_challenge(
    app: web::Data<Application>,
    request: HttpRequest,
    input: web::Json<ChallengeInput>,
) -> ApiResult {
    let session = authenticated(&app, &request).await?;
    Ok(HttpResponse::Ok().json(
        app.accounts
            .begin_link_with_callback(
                &session,
                input.chain,
                &input.address,
                input.callback_url.as_deref(),
            )
            .await
            .map_err(ApiError::action)?,
    ))
}
pub(super) async fn link_wallet(
    app: web::Data<Application>,
    request: HttpRequest,
    input: web::Json<ProofInput>,
) -> ApiResult {
    let session = authenticated(&app, &request).await?;
    app.accounts
        .complete_link(
            &session,
            input.challenge,
            &input.signature,
            input.public_key.as_deref(),
        )
        .await
        .map_err(ApiError::action)?;
    Ok(HttpResponse::Ok().json(
        account_json(&app, &session)
            .await
            .map_err(ApiError::service)?,
    ))
}
