//! C S065: POST for encrypted paid images, never a direct/public S3 proxy.
//! This route authenticates session + CSRF then C enforces D -> B -> G -> D.
use super::*;
use tor_contracts::paid_delivery::ObjectId;
use tor_reading::services::paid_delivery::{issue_paid_admin_asset_grant, issue_paid_asset_grant};

pub(super) async fn grant_asset(
    State(state): State<GrantState>,
    Path((raw_chapter, raw_object)): Path<(String, String)>,
    headers: HeaderMap,
    Json(body): Json<GrantBody>,
) -> Response {
    let (session, actor) = match super::request_auth::authenticate_grant(&state, &headers).await {
        Ok(authenticated) => authenticated,
        Err(status) => return deny(status),
    };
    let (Some(chapter_id), Ok(object_id), Ok(actor_id)) = (
        positive(&raw_chapter),
        ObjectId::parse(&raw_object),
        PositiveId::new(actor),
    ) else {
        return deny(StatusCode::NOT_FOUND);
    };
    let purchase_id = match body.purchase_id.as_deref() {
        Some(raw) => match positive(raw) {
            Some(value) => Some(value),
            None => return deny(StatusCode::UNPROCESSABLE_ENTITY),
        },
        None => None,
    };
    if matches!(body.source, Some(GrantSource::Layer)) && !state.layer_reader_enabled {
        return deny(StatusCode::NOT_FOUND);
    }
    if body.source.is_some() && purchase_id.is_some() {
        return deny(StatusCode::UNPROCESSABLE_ENTITY);
    }
    let admin = PgAdministratorReaderEntitlements::new(
        state.accounts.pool().clone(),
        session_token_digest(&session),
    );
    let authority: &dyn EntitlementPort = match body.source {
        Some(GrantSource::Layer) => &state.layer,
        Some(GrantSource::Administrator) => &admin,
        None => &state.entitlement,
    };
    let assets: &dyn tor_contracts::paid_delivery::ChapterAssetPort =
        if matches!(body.source, Some(GrantSource::Administrator)) {
            &state.admin_assets
        } else {
            &state.assets
        };
    if !state.rate_limits.lock().await.allow(actor, Instant::now()) {
        return deny(StatusCode::TOO_MANY_REQUESTS);
    }
    let Ok(_permit) = state.concurrent.clone().try_acquire_owned() else {
        return deny(StatusCode::TOO_MANY_REQUESTS);
    };
    let result = if matches!(body.source, Some(GrantSource::Administrator)) {
        issue_paid_admin_asset_grant(
            authority,
            assets,
            state.storage.as_ref(),
            Actor { user_id: actor_id },
            chapter_id,
            &object_id,
        )
        .await
    } else {
        issue_paid_asset_grant(
            authority,
            assets,
            state.storage.as_ref(),
            Actor { user_id: actor_id },
            chapter_id,
            purchase_id,
            &object_id,
        )
        .await
    };
    match result {
        Ok(grant) => {
            let Some(version) = positive(&grant.version_id) else {
                return deny(StatusCode::SERVICE_UNAVAILABLE);
            };
            if body.source.is_none() {
                if let Err(error) = state
                    .first_direct_grant
                    .record(
                        actor_id,
                        chapter_id,
                        version,
                        purchase_id,
                        DirectGrantKind::Asset,
                    )
                    .await
                {
                    return deny(status(error));
                }
            }
            // Persist audit before returning a signed ciphertext image URL.
            if matches!(body.source, Some(GrantSource::Administrator)) {
                if let Err(error) = state
                    .admin_audit
                    .record(
                        &session_token_digest(&session),
                        actor_id,
                        chapter_id,
                        version,
                        AdminReadAction::SealedAssetGrant,
                    )
                    .await
                {
                    return deny(status(error));
                }
            }
            let mut response = private_response(Json(grant), &[]);
            response.headers_mut().insert(
                header::CACHE_CONTROL,
                HeaderValue::from_static("private, no-store"),
            );
            response.headers_mut().insert(
                header::REFERRER_POLICY,
                HeaderValue::from_static("no-referrer"),
            );
            response
                .headers_mut()
                .insert("x-robots-tag", HeaderValue::from_static("noindex"));
            response
        }
        Err(error) => deny(status(error)),
    }
}
