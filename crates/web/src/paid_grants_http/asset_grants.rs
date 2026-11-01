//! C S065: POST for encrypted paid images, never a direct/public S3 proxy.
//! This route authenticates session + CSRF then C enforces D -> B -> G -> D.
use super::*;
use tor_contracts::paid_delivery::ObjectId;
use tor_reading::services::paid_delivery::issue_paid_asset_grant;

pub(super) async fn grant_asset(
    State(state): State<GrantState>,
    Path((raw_chapter, raw_object)): Path<(String, String)>,
    headers: HeaderMap,
    Json(body): Json<GrantBody>,
) -> Response {
    let Some(session) = state.cookies.session_token(&headers) else {
        return deny(StatusCode::UNAUTHORIZED);
    };
    let actor = match state
        .accounts
        .active_session_user(&session_token_digest(&session))
        .await
    {
        Ok(Some(actor)) => actor,
        Ok(None) => return deny(StatusCode::UNAUTHORIZED),
        Err(_) => return deny(StatusCode::SERVICE_UNAVAILABLE),
    };
    if headers.get(header::ORIGIN).and_then(|v| v.to_str().ok())
        != Some(state.cookies.origin.as_str())
        || !state.cookies.same_origin(&headers)
        || !headers
            .get("x-csrf-token")
            .and_then(|v| v.to_str().ok())
            .is_some_and(|csrf| state.cookies.valid_csrf(&headers, csrf))
    {
        return deny(StatusCode::FORBIDDEN);
    }
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
    // Chapter images inherit the identical edition-specific D authority.
    if body.source.is_some() && purchase_id.is_some() {
        return deny(StatusCode::UNPROCESSABLE_ENTITY);
    }
    if !state.rate_limits.lock().await.allow(actor, Instant::now()) {
        return deny(StatusCode::TOO_MANY_REQUESTS);
    }
    let Ok(_permit) = state.concurrent.clone().try_acquire_owned() else {
        return deny(StatusCode::TOO_MANY_REQUESTS);
    };
    // Paid illustrations use exactly the same D-owned entitlement source
    // as a chapter ciphertext grant. The default source can fall back from
    // an unowned direct purchase to a valid Layer term.
    let selected_layer = if body.source.is_some() {
        true
    } else {
        match state
            .entitlement
            .resolve_with_source(Actor { user_id: actor_id }, chapter_id, purchase_id)
            .await
        {
            Ok((_, ReaderEntitlementSource::Direct)) => false,
            Ok((_, ReaderEntitlementSource::Layer)) => true,
            Err(error) => return deny(status(error)),
        }
    };
    let authority: &dyn EntitlementPort = if selected_layer {
        &state.layer
    } else {
        &state.direct
    };
    let result = issue_paid_asset_grant(
        authority,
        &state.assets,
        state.storage.as_ref(),
        Actor { user_id: actor_id },
        chapter_id,
        purchase_id,
        &object_id,
    )
    .await;
    match result {
        Ok(grant) => {
            let Some(version) = positive(&grant.version_id) else {
                return deny(StatusCode::SERVICE_UNAVAILABLE);
            };
            // Only a direct purchase creates the D-06 first exposure.
            // A Layer-only illustration grant has a separate paid term
            // and must never forge direct-purchase finance evidence.
            if !selected_layer {
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
