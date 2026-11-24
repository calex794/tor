//! C-12/S065: independent D/A entitlement and B/G signing for sealed paid text.
use super::*;

pub(super) async fn grant(
    State(state): State<GrantState>,
    Path(raw_chapter): Path<String>,
    headers: HeaderMap,
    Json(body): Json<GrantBody>,
) -> Response {
    let (session, actor) = match super::request_auth::authenticate_grant(&state, &headers).await {
        Ok(authenticated) => authenticated,
        Err(status) => return deny(status),
    };
    let Some(chapter_id) = positive(&raw_chapter) else {
        return deny(StatusCode::NOT_FOUND);
    };
    let purchase_id = match body.purchase_id.as_deref() {
        Some(value) => match positive(value) {
            Some(id) => Some(id),
            None => return deny(StatusCode::UNPROCESSABLE_ENTITY),
        },
        None => None,
    };
    let Ok(actor_id) = PositiveId::new(actor) else {
        return deny(StatusCode::SERVICE_UNAVAILABLE);
    };
    if !state.rate_limits.lock().await.allow(actor, Instant::now()) {
        return deny(StatusCode::TOO_MANY_REQUESTS);
    }

    let Ok(_permit) = state.concurrent.clone().try_acquire_owned() else {
        return deny(StatusCode::TOO_MANY_REQUESTS);
    };

    // The paid S3 reader must not silently activate Layer subscriptions.
    // Direct V1 buyers retain their original grant path when Layer is off.
    if matches!(body.source, Some(GrantSource::Layer)) && !state.layer_reader_enabled {
        return deny(StatusCode::NOT_FOUND);
    }
    // A Layer term uses a distinct D authority. Explicit direct purchase IDs
    // cannot be combined with Layer rights, preventing provenance confusion.
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
    let versions: &dyn ChapterVersionPort =
        if matches!(body.source, Some(GrantSource::Administrator)) {
            &state.admin_versions
        } else {
            &state.versions
        };
    // C rechecks the same D authority after signing; the browser never sees
    // plaintext or a raw object key.
    let result = issue_paid_chapter_grant(
        authority,
        versions,
        state.storage.as_ref(),
        Actor { user_id: actor_id },
        chapter_id,
        purchase_id,
    )
    .await;
    match result {
        Ok(grant) => {
            let Some(version) = positive(&grant.version_id) else {
                return deny(StatusCode::SERVICE_UNAVAILABLE);
            };
            // First exposure belongs to a *direct* pinned purchase only.
            // Layer CURRENT grants have no direct purchase fact to expose.
            if body.source.is_none() {
                if let Err(error) = state
                    .first_direct_grant
                    .record(
                        actor_id,
                        chapter_id,
                        version,
                        purchase_id,
                        DirectGrantKind::Chapter,
                    )
                    .await
                {
                    return deny(status(error));
                }
            }
            // A successful administrator-only ciphertext grant must have a
            // durable metadata audit. No buyer or Layer event is recorded.
            if matches!(body.source, Some(GrantSource::Administrator)) {
                if let Err(error) = state
                    .admin_audit
                    .record(
                        &session_token_digest(&session),
                        actor_id,
                        chapter_id,
                        version,
                        AdminReadAction::SealedChapterGrant,
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
