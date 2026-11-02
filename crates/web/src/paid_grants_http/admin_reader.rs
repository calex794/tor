//! Site-administrator preview, separate from D buyer and Layer entitlements.
use super::*;
use serde::Deserialize;
use tor_books::repositories::admin_reading::PgAdministratorReading;

#[derive(Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub(super) struct AdminReaderQuery {
    version_id: Option<i64>,
}

pub(super) async fn read(
    State(state): State<GrantState>,
    Path(raw_chapter): Path<String>,
    Query(query): Query<AdminReaderQuery>,
    headers: HeaderMap,
) -> Response {
    let Some(session) = state.cookies.session_token(&headers) else {
        return deny(StatusCode::UNAUTHORIZED);
    };
    let actor = match state
        .accounts
        .session_actor(&session_token_digest(&session))
        .await
    {
        Ok(Some(actor)) if actor.administrator => actor.actor.user_id.get(),
        Ok(Some(_)) => return deny(StatusCode::FORBIDDEN),
        Ok(None) => return deny(StatusCode::UNAUTHORIZED),
        Err(_) => return deny(StatusCode::SERVICE_UNAVAILABLE),
    };
    let Some(chapter_id) = positive(&raw_chapter) else {
        return deny(StatusCode::NOT_FOUND);
    };
    // B repeats the live role check within its content SELECT. A forged
    // session, stale role or version_id cannot expose any chapter bytes.
    let repository = PgAdministratorReading::new(state.accounts.pool().clone());
    let chapter = match repository
        .read(actor, chapter_id.get(), query.version_id)
        .await
    {
        Ok(Some(chapter)) => chapter,
        Ok(None) => return deny(StatusCode::NOT_FOUND),
        Err(_) => return deny(StatusCode::SERVICE_UNAVAILABLE),
    };
    if !tor_reading::domain::renderer::valid_reader_length(&chapter.body_markdown) {
        return deny(StatusCode::PAYLOAD_TOO_LARGE);
    }
    let mut response = private_response(pages::administrator_chapter_view(&chapter), &[]);
    response.headers_mut().insert(
        "content-security-policy",
        HeaderValue::from_static(
            "default-src 'none'; script-src 'none'; connect-src 'none';              style-src 'self'; img-src 'none'; form-action 'none';              base-uri 'none'; frame-ancestors 'none'",
        ),
    );
    response.headers_mut().insert(
        header::REFERRER_POLICY,
        HeaderValue::from_static("no-referrer"),
    );
    response.headers_mut().insert(
        "x-robots-tag",
        HeaderValue::from_static("noindex, nofollow"),
    );
    response
}
