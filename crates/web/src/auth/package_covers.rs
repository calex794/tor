//! Package-cover publication, owner-private preview and moderator decision.
use super::{deny, session_user, AuthState};
use crate::{auth_cookies::private_response, pages};
use axum::{
    extract::{Form, Path, State},
    http::{header, HeaderMap, StatusCode},
    response::{Html, IntoResponse, Redirect, Response},
    routing::{get, post},
    Router,
};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use tor_accounts::services::security::session_token_digest;
use tor_commerce::repositories::layer_covers::{PackageCoverCandidate, PgLayerCovers};
use tor_contracts::paid_delivery::PositiveId;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ReviewForm {
    csrf_token: String,
    action: String,
    confirmed: Option<String>,
    reason: Option<String>,
}

pub(super) fn routes() -> Router<AuthState> {
    Router::new()
        .route("/layers/{layer_id}/cover/{variant}", get(public_cover))
        .route("/me/layers/{layer_id}/cover/{mode}", get(owner_preview))
        .route("/moderation/layers/covers", get(queue))
        .route("/moderation/layers/covers/{id}/preview", get(moderator_preview))
        .route("/moderation/layers/covers/{id}/decision", post(decide))
}

fn repository(state: &AuthState) -> PgLayerCovers {
    PgLayerCovers::new(state.accounts.pool().clone())
}

async fn actor(state: &AuthState, headers: &HeaderMap) -> Result<PositiveId, StatusCode> {
    let token = state.cookies.session_token(headers).ok_or(StatusCode::UNAUTHORIZED)?;
    let digest = session_token_digest(&token);
    let actor = state
        .accounts
        .session_actor(&digest)
        .await
        .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?
        .ok_or(StatusCode::UNAUTHORIZED)?;
    if !(actor.moderator || actor.administrator) {
        return Err(StatusCode::FORBIDDEN);
    }
    let user = state
        .accounts
        .active_session_user(&digest)
        .await
        .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?
        .ok_or(StatusCode::UNAUTHORIZED)?;
    PositiveId::new(user).map_err(|_| StatusCode::UNAUTHORIZED)
}

async fn read_image(
    state: &AuthState,
    cover: &PackageCoverCandidate,
    variant: &str,
) -> Result<Vec<u8>, StatusCode> {
    let Some(storage) = state.cover_preview_storage.as_ref() else {
        return Err(StatusCode::NOT_FOUND);
    };
    let (key, size, digest) = match variant {
        "main" => (&cover.main_key, cover.main_bytes, cover.main_sha256),
        "card" => (&cover.card_key, cover.card_bytes, cover.card_sha256),
        _ => return Err(StatusCode::NOT_FOUND),
    };
    if !(1..=8 * 1024 * 1024).contains(&size) {
        return Err(StatusCode::SERVICE_UNAVAILABLE);
    }
    let bytes = storage
        .get_bounded(key, 8 * 1024 * 1024)
        .await
        .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?;
    let hash: [u8; 32] = Sha256::digest(&bytes).into();
    if hash != digest || bytes.len() as i64 != size {
        return Err(StatusCode::SERVICE_UNAVAILABLE);
    }
    Ok(bytes)
}

fn image_response(bytes: Vec<u8>, public: bool) -> Response {
    let cache = if public {
        "public, max-age=30, must-revalidate"
    } else {
        "private, no-store"
    };
    (
        [
            (header::CONTENT_TYPE, "image/webp"),
            (header::CACHE_CONTROL, cache),
            (header::X_CONTENT_TYPE_OPTIONS, "nosniff"),
        ],
        bytes,
    )
        .into_response()
}

async fn public_cover(
    State(state): State<AuthState>,
    Path((raw_layer, variant)): Path<(i64, String)>,
) -> Response {
    let Ok(layer) = PositiveId::new(raw_layer) else {
        return deny(StatusCode::NOT_FOUND);
    };
    let kind = match variant.as_str() {
        "main.webp" => "main",
        "card.webp" => "card",
        _ => return deny(StatusCode::NOT_FOUND),
    };
    let repo = repository(&state);
    let cover = match repo.public_current(layer).await {
        Ok(Some(cover)) => cover,
        Ok(None) => return deny(StatusCode::NOT_FOUND),
        Err(_) => return deny(StatusCode::SERVICE_UNAVAILABLE),
    };
    let image = match read_image(&state, &cover, kind).await {
        Ok(image) => image,
        Err(error) => return deny(error),
    };
    // Recheck moderation and public layer status after the storage read.
    if repo.public_current(layer).await.ok().flatten().as_ref() != Some(&cover) {
        return deny(StatusCode::NOT_FOUND);
    }
    image_response(image, true)
}

async fn owner_preview(
    State(state): State<AuthState>,
    Path((raw_layer, mode)): Path<(i64, String)>,
    headers: HeaderMap,
) -> Response {
    let raw_owner = match session_user(&state, &headers).await {
        Ok(Some(id)) => id,
        Ok(None) => return deny(StatusCode::UNAUTHORIZED),
        Err(error) => return deny(error),
    };
    let (Ok(owner), Ok(layer)) = (PositiveId::new(raw_owner), PositiveId::new(raw_layer)) else {
        return deny(StatusCode::NOT_FOUND);
    };
    let repo = repository(&state);
    let candidate = match repo.owner_view(owner, layer).await {
        Ok((approved, pending)) => match mode.as_str() {
            "preview" => pending,
            "approved" => approved,
            _ => return deny(StatusCode::NOT_FOUND),
        },
        Err(_) => return deny(StatusCode::NOT_FOUND),
    };
    let Some(candidate) = candidate else {
        return deny(StatusCode::NOT_FOUND);
    };
    let image = match read_image(&state, &candidate, "main").await {
        Ok(image) => image,
        Err(error) => return deny(error),
    };
    let unchanged = repo.owner_view(owner, layer).await.ok()
        .and_then(|(approved, pending)| match mode.as_str() {
            "preview" => pending,
            _ => approved,
        })
        .as_ref() == Some(&candidate);
    if !unchanged {
        return deny(StatusCode::NOT_FOUND);
    }
    private_response(image_response(image, false), &[])
}

async fn queue(State(state): State<AuthState>, headers: HeaderMap) -> Response {
    if let Err(status) = actor(&state, &headers).await {
        return deny(status);
    }
    let rows = match repository(&state).pending_queue(40).await {
        Ok(rows) => rows,
        Err(_) => return deny(StatusCode::SERVICE_UNAVAILABLE),
    };
    let (csrf, cookies) = state.cookies.csrf_for_page(&headers);
    let mut html = String::new();
    for item in rows {
        let id = item.candidate.id;
        let layer = item.candidate.layer_id;
        let title = pages::escape_html(&item.layer_title);
        html.push_str(&format!(
            r#"<article class="me-now-card">
               <h2>Пакет №{layer} · {title}</h2>
               <img src="/moderation/layers/covers/{id}/preview"
                 alt="Кандидат обложки пакета" width="150" height="220" loading="lazy">
               <form action="/moderation/layers/covers/{id}/decision" method="post">
                 <input type="hidden" name="csrf_token" value="{csrf}">
                 <label for="package-cover-reason-{id}">Причина отклонения</label>
                 <textarea id="package-cover-reason-{id}" maxlength="2000"
                    name="reason" rows="2"></textarea>
                 <label><input type="checkbox" name="confirmed" value="yes" required>
                   Подтверждаю проверку изображения и решение</label>
                 <button type="submit" name="action" value="approve">Одобрить</button>
                 <button type="submit" name="action" value="reject">Отклонить</button>
               </form>
               </article>"#,
            csrf = pages::escape_html(&csrf)
        ));
    }
    if html.is_empty() {
        html.push_str("<p>Ожидающих проверки обложек пакетов нет.</p>");
    }
    let contents = format!(
        r#"<section class="me-page"><div class="container">
           <h1>Модерация обложек пакетов</h1>
           <p>После одобрения замены ранее опубликованная обложка перестанет выдаваться.</p>
           {html}
           <p><a href="/moderation/covers">Модерация обложек книг</a></p>
           </div></section>"#
    );
    private_response(
        pages::private_account_layout(
            "Модерация обложек пакетов",
            "Закрытая очередь обложек пакетов",
            &contents,
        ),
        &cookies,
    )
}

async fn moderator_preview(
    State(state): State<AuthState>,
    Path(raw_candidate): Path<i64>,
    headers: HeaderMap,
) -> Response {
    if let Err(error) = actor(&state, &headers).await {
        return deny(error);
    }
    let Ok(candidate) = PositiveId::new(raw_candidate) else {
        return deny(StatusCode::NOT_FOUND);
    };
    let repo = repository(&state);
    let pending = match repo.pending_by_id(candidate).await {
        Ok(Some(pending)) => pending,
        Ok(None) => return deny(StatusCode::NOT_FOUND),
        Err(_) => return deny(StatusCode::SERVICE_UNAVAILABLE),
    };
    let image = match read_image(&state, &pending, "main").await {
        Ok(image) => image,
        Err(error) => return deny(error),
    };
    if repo.pending_by_id(candidate).await.ok().flatten().as_ref() != Some(&pending) {
        return deny(StatusCode::NOT_FOUND);
    }
    private_response(image_response(image, false), &[])
}

async fn decide(
    State(state): State<AuthState>,
    Path(raw_candidate): Path<i64>,
    headers: HeaderMap,
    Form(form): Form<ReviewForm>,
) -> Response {
    if !state.cookies.same_origin(&headers)
        || !state.cookies.valid_csrf(&headers, &form.csrf_token)
    {
        return deny(StatusCode::FORBIDDEN);
    }
    let reviewer = match actor(&state, &headers).await {
        Ok(reviewer) => reviewer,
        Err(error) => return deny(error),
    };
    let Ok(candidate) = PositiveId::new(raw_candidate) else {
        return deny(StatusCode::NOT_FOUND);
    };
    let approve = match form.action.as_str() {
        "approve" => true,
        "reject" => false,
        _ => return deny(StatusCode::UNPROCESSABLE_ENTITY),
    };
    if form.confirmed.as_deref() != Some("yes") {
        return deny(StatusCode::UNPROCESSABLE_ENTITY);
    }
    let reason = form.reason.as_deref().unwrap_or("").trim();
    let repo = repository(&state);
    // Require two verified S3 variants to exist before approval.
    if approve {
        let pending = match repo.pending_by_id(candidate).await {
            Ok(Some(pending)) => pending,
            Ok(None) => return deny(StatusCode::NOT_FOUND),
            Err(_) => return deny(StatusCode::SERVICE_UNAVAILABLE),
        };
        if read_image(&state, &pending, "card").await.is_err()
            || read_image(&state, &pending, "main").await.is_err()
        {
            return deny(StatusCode::SERVICE_UNAVAILABLE);
        }
    }
    match repo.review(reviewer, candidate, approve, reason).await {
        Ok(()) => private_response(Redirect::to("/moderation/layers/covers"), &[]),
        Err(tor_commerce::domain::layers::LayerError::Invalid) => {
            deny(StatusCode::UNPROCESSABLE_ENTITY)
        }
        Err(tor_commerce::domain::layers::LayerError::StaleOffer) => {
            deny(StatusCode::CONFLICT)
        }
        Err(_) => deny(StatusCode::SERVICE_UNAVAILABLE),
    }
}
