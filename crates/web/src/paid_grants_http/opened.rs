//! Client-reported verified render AFTER D -> B -> G, not signed issuance.
//! Session/Origin/CSRF + D-owned locks enforce an existing paid grant.
//! A missing acknowledgement never proves that content was not consumed.
use super::*;
use serde::Serialize;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct OpenBody {
    purchase_id: Option<String>,
    source: Option<GrantSource>,
    version_id: String,
}

#[derive(Serialize)]
struct OpenReceipt {
    status: &'static str,
}

pub(super) async fn confirm_open(
    State(state): State<GrantState>,
    Path(raw_chapter): Path<String>,
    headers: HeaderMap,
    Json(body): Json<OpenBody>,
) -> Response {
    if state.reader_s3_origin.is_none() {
        return deny(StatusCode::NOT_FOUND);
    }
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
    let (Some(chapter), Some(version), Ok(actor_id)) = (
        positive(&raw_chapter),
        positive(&body.version_id),
        PositiveId::new(actor),
    ) else {
        return deny(StatusCode::UNPROCESSABLE_ENTITY);
    };
    if !state.rate_limits.lock().await.allow(actor, Instant::now()) {
        return deny(StatusCode::TOO_MANY_REQUESTS);
    }
    // Explicit source and purchase prevent accidental direct/Layer fallback.
    let result = match (body.source, body.purchase_id.as_deref()) {
        (Some(GrantSource::Layer), None) => {
            state
                .first_layer_grant
                .confirm_rendered(actor_id, chapter, version)
                .await
        }
        (None, Some(raw_purchase)) => {
            let Some(purchase) = positive(raw_purchase) else {
                return deny(StatusCode::UNPROCESSABLE_ENTITY);
            };
            state
                .first_direct_grant
                .confirm_rendered(actor_id, chapter, version, purchase)
                .await
        }
        _ => return deny(StatusCode::UNPROCESSABLE_ENTITY),
    };
    if let Err(error) = result {
        return deny(status(error));
    }
    // Replays do not update the original timestamp or reveal timing.
    private_response(Json(OpenReceipt { status: "recorded" }), &[])
}
