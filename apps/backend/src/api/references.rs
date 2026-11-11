//! Immutable human-readable text behind an on-chain purpose/report digest.
//! Knowing or reusing a digest does not grant access to its original text.
use actix_web::{HttpRequest, HttpResponse, http::StatusCode, web};
use serde::Deserialize;
use serde_json::json;
use sha2::{Digest, Sha256};
use sqlx::FromRow;
use uuid::Uuid;

use super::{ApiError, ApiResult, authenticated, deployment};
use crate::{application::Application, config::word};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct CreateInput {
    content: String,
}
#[derive(FromRow)]
struct Reference {
    author_profile: Uuid,
    content: String,
}

pub(super) async fn create(
    app: web::Data<Application>,
    request: HttpRequest,
    key: web::Path<String>,
    input: web::Json<CreateInput>,
) -> ApiResult {
    let session = authenticated(&app, &request).await?;
    deployment(&app, &key)?;
    let profile = app
        .accounts
        .session(&session)
        .await
        .map_err(ApiError::service)?
        .profile;
    let content = input.content.trim();
    if content.is_empty()
        || content.len() > 16_000
        || content
            .chars()
            .any(|character| character.is_control() && !matches!(character, '\n' | '\r' | '\t'))
    {
        return Err(ApiError::invalid(
            "Write between 1 and 16,000 bytes of readable text.",
        ));
    }
    let digest = hex::encode(Sha256::digest(content.as_bytes()));
    let inserted = sqlx::query(
        "INSERT INTO content_references(project,digest,author_profile,content) VALUES($1,$2,$3,$4) \
         ON CONFLICT DO NOTHING",
    )
    .bind(key.as_str())
    .bind(&digest)
    .bind(profile.id)
    .bind(content)
    .execute(&app.indexer.store.pool)
    .await
    .map_err(|error| ApiError::service(error.into()))?
    .rows_affected();
    if inserted == 0 {
        let existing: Reference = sqlx::query_as(
            "SELECT author_profile,content FROM content_references WHERE project=$1 AND digest=$2",
        )
        .bind(key.as_str())
        .bind(&digest)
        .fetch_one(&app.indexer.store.pool)
        .await
        .map_err(|error| ApiError::service(error.into()))?;
        if existing.author_profile != profile.id || existing.content != content {
            return Err(ApiError::new(
                StatusCode::CONFLICT,
                "reference_exists",
                "This exact reference already belongs to another author. Add specific context to your description.",
            ));
        }
    }
    Ok(HttpResponse::Ok().json(json!({"digest":digest,"content":content})))
}

pub(super) async fn read(
    app: web::Data<Application>,
    request: HttpRequest,
    path: web::Path<(String, String)>,
) -> ApiResult {
    let session = authenticated(&app, &request).await?;
    let (key, digest) = path.into_inner();
    let project = deployment(&app, &key)?;
    let digest =
        hex::encode(word(&digest).map_err(|_| ApiError::invalid("Invalid reference digest."))?);
    let reference: Option<Reference> = sqlx::query_as(
        "SELECT author_profile,content FROM content_references WHERE project=$1 AND digest=$2",
    )
    .bind(&key)
    .bind(&digest)
    .fetch_optional(&app.indexer.store.pool)
    .await
    .map_err(|error| ApiError::service(error.into()))?;
    let reference = reference.ok_or_else(|| {
        ApiError::new(
            StatusCode::NOT_FOUND,
            "reference_not_found",
            "The description for this on-chain reference is not available here.",
        )
    })?;
    let profile = app
        .accounts
        .session(&session)
        .await
        .map_err(ApiError::service)?
        .profile;
    if reference.author_profile != profile.id {
        // Management access follows current finalized authority, including
        // immediate role changes; a stale read projection cannot grant access.
        let block = app
            .indexer
            .near
            .finalized_block()
            .await
            .map_err(ApiError::service)?;
        let state = app
            .indexer
            .near
            .coordinator_state(&project.coordinator, &block)
            .await
            .map_err(ApiError::service)?;
        if word(&state.project_id).map_err(ApiError::service)?
            != word(&project.project_id).map_err(ApiError::service)?
        {
            return Err(ApiError::new(
                StatusCode::SERVICE_UNAVAILABLE,
                "deployment_mismatch",
                "The current project identity could not be verified.",
            ));
        }
        let owner = app
            .accounts
            .has_current_near_wallet(&session, &state.owner_id)
            .await
            .map_err(ApiError::service)?;
        let authority = if owner {
            false
        } else {
            app.accounts
                .has_current_near_wallet(&session, &state.authority_id)
                .await
                .map_err(ApiError::service)?
        };
        let permitted = owner || authority;
        if !permitted {
            return Err(ApiError::new(
                StatusCode::FORBIDDEN,
                "reference_forbidden",
                "This description is available to its author and current project management.",
            ));
        }
    }
    Ok(HttpResponse::Ok().json(json!({"digest":digest,"content":reference.content})))
}
