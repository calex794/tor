//! H/S065: first complete author path for ONE paid illustration.
//! The final B body and its private asset UUID are fixed before SaveChapter.
//! No public media URL, client-selected UUID, inline script or new S3 route.
use crate::{
    auth_cookies::{private_response, CookiePolicy},
    author_chapter_http::PaidChapterPublisher,
    author_chapter_pages,
};
use axum::{
    extract::{DefaultBodyLimit, Multipart, Path, State},
    http::{HeaderMap, StatusCode},
    response::{Redirect, Response},
    routing::get,
    Router,
};
use sha2::{Digest, Sha256};
use std::{fmt::Write, sync::Arc};
use tokio::sync::Semaphore;
use tor_accounts::{repositories::PgAccounts, services::security::session_token_digest};
use tor_books::{
    repositories::postgres::PgAuthoring,
    services::authoring::{save_chapter, AuthorError, SaveChapterRequest},
};
use tor_contracts::paid_delivery::ObjectId;
use tor_ingest::services::image_pipeline::{validate_cover_source, MAX_SOURCE_BYTES};

const MAX_REQUEST_BYTES: usize = MAX_SOURCE_BYTES + 192 * 1024;

#[derive(Clone)]
struct StateAuthorImage {
    accounts: PgAccounts,
    authoring: PgAuthoring,
    cookies: CookiePolicy,
    publisher: Arc<dyn PaidChapterPublisher>,
    pending: Arc<Semaphore>,
}

pub(crate) fn router(
    accounts: PgAccounts,
    cookies: CookiePolicy,
    publisher: Arc<dyn PaidChapterPublisher>,
) -> Router {
    let state = StateAuthorImage {
        authoring: PgAuthoring::new(accounts.pool().clone()),
        accounts,
        cookies,
        publisher,
        pending: Arc::new(Semaphore::new(2)),
    };
    Router::new()
        .route(
            "/me/books/{id}/chapters/illustrated",
            get(show).post(create),
        )
        .layer(DefaultBodyLimit::max(MAX_REQUEST_BYTES))
        .with_state(state)
}

fn deny(code: StatusCode) -> Response {
    private_response(code, &[])
}

async fn owner(
    state: &StateAuthorImage,
    headers: &HeaderMap,
    book: i64,
) -> Result<Option<i64>, StatusCode> {
    if book <= 0 {
        return Err(StatusCode::NOT_FOUND);
    }
    let Some(token) = state.cookies.session_token(headers) else {
        return Ok(None);
    };
    let actor = state
        .accounts
        .active_session_user(&session_token_digest(&token))
        .await
        .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?;
    let Some(actor) = actor else {
        return Ok(None);
    };
    match state.authoring.owned_book_is_public(actor, book).await {
        Ok(true) => Ok(Some(actor)),
        Ok(false) | Err(AuthorError::Missing) => Err(StatusCode::NOT_FOUND),
        Err(_) => Err(StatusCode::SERVICE_UNAVAILABLE),
    }
}

/// Stable, unpredictable-to-outsiders UUIDv4-style identity derived from the
/// server-issued 256-bit author command. This is NOT a grant or entitlement.
pub(crate) fn stable_image_id(actor: i64, book: i64, command_key: &str) -> ObjectId {
    let mut hash = Sha256::new();
    hash.update(b"tor.author.single-asset.v1");
    hash.update(actor.to_be_bytes());
    hash.update(book.to_be_bytes());
    hash.update(command_key.as_bytes());
    let digest = hash.finalize();
    let mut uuid = [0_u8; 16];
    uuid.copy_from_slice(&digest[..16]);
    uuid[6] = (uuid[6] & 0x0f) | 0x40;
    uuid[8] = (uuid[8] & 0x3f) | 0x80;
    let mut text = String::with_capacity(36);
    for (pos, byte) in uuid.iter().enumerate() {
        if matches!(pos, 4 | 6 | 8 | 10) {
            text.push('-');
        }
        write!(&mut text, "{byte:02x}").expect("write to String");
    }
    ObjectId::parse(&text).expect("canonical 36-character UUID")
}

async fn show(
    State(state): State<StateAuthorImage>,
    Path(book): Path<i64>,
    headers: HeaderMap,
) -> Response {
    match owner(&state, &headers, book).await {
        Ok(Some(_)) => (),
        Ok(None) => return private_response(Redirect::to("/login"), &[]),
        Err(code) => return deny(code),
    }
    let (csrf, cookies) = state.cookies.csrf_for_page(&headers);
    private_response(
        author_chapter_pages::illustrated_chapter(
            book,
            &csrf,
            &tor_accounts::services::security::generate_session_token(),
        ),
        &cookies,
    )
}

#[derive(Default)]
struct ImageForm {
    csrf: Option<String>,
    command: Option<String>,
    number: Option<i32>,
    title: Option<String>,
    price: Option<i64>,
    body: Option<String>,
    image: Option<(String, Vec<u8>)>,
}

async fn read_form(
    mut parts: Multipart,
    headers: &HeaderMap,
    cookies: &CookiePolicy,
) -> Result<ImageForm, StatusCode> {
    let mut parsed = ImageForm::default();
    let mut count = 0;
    loop {
        let field = match parts.next_field().await {
            Ok(Some(field)) => field,
            Ok(None) => break,
            Err(_) => return Err(StatusCode::BAD_REQUEST),
        };
        count += 1;
        if count > 7 {
            return Err(StatusCode::BAD_REQUEST);
        }
        let name = field.name().unwrap_or("").to_owned();
        // CSRF must precede any image bytes. The browser's form renders it first.
        if parsed.csrf.is_none() && name != "csrf_token" {
            return Err(StatusCode::FORBIDDEN);
        }
        match name.as_str() {
            "csrf_token" if parsed.csrf.is_none() => {
                let value = field.text().await.map_err(|_| StatusCode::BAD_REQUEST)?;
                if !cookies.valid_csrf(headers, &value) {
                    return Err(StatusCode::FORBIDDEN);
                }
                parsed.csrf = Some(value);
            }
            "command_key" if parsed.command.is_none() => {
                let value = field.text().await.map_err(|_| StatusCode::BAD_REQUEST)?;
                if value.len() > 128 {
                    return Err(StatusCode::BAD_REQUEST);
                }
                parsed.command = Some(value);
            }
            "number" if parsed.number.is_none() => {
                parsed.number = Some(
                    field
                        .text()
                        .await
                        .map_err(|_| StatusCode::BAD_REQUEST)?
                        .parse()
                        .map_err(|_| StatusCode::BAD_REQUEST)?,
                );
            }
            "title" if parsed.title.is_none() => {
                let value = field.text().await.map_err(|_| StatusCode::BAD_REQUEST)?;
                if value.len() > 800 {
                    return Err(StatusCode::BAD_REQUEST);
                }
                parsed.title = Some(value);
            }
            "price_amount" if parsed.price.is_none() => {
                parsed.price = Some(
                    field
                        .text()
                        .await
                        .map_err(|_| StatusCode::BAD_REQUEST)?
                        .parse()
                        .map_err(|_| StatusCode::BAD_REQUEST)?,
                );
            }
            "body" if parsed.body.is_none() => {
                let body = field.text().await.map_err(|_| StatusCode::BAD_REQUEST)?;
                if body.len() > 200_000 {
                    return Err(StatusCode::PAYLOAD_TOO_LARGE);
                }
                parsed.body = Some(body);
            }
            "illustration" if parsed.image.is_none() => {
                let mime = match field.content_type() {
                    Some("image/png") => "image/png",
                    Some("image/jpeg") => "image/jpeg",
                    Some("image/webp") => "image/webp",
                    _ => return Err(StatusCode::UNSUPPORTED_MEDIA_TYPE),
                };
                let bytes = field.bytes().await.map_err(|_| StatusCode::BAD_REQUEST)?;
                if bytes.is_empty() || bytes.len() > MAX_SOURCE_BYTES {
                    return Err(StatusCode::PAYLOAD_TOO_LARGE);
                }
                parsed.image = Some((mime.to_owned(), bytes.to_vec()));
            }
            _ => return Err(StatusCode::BAD_REQUEST),
        }
    }
    if count != 7 || !cookies.valid_csrf(headers, parsed.csrf.as_deref().unwrap_or("")) {
        return Err(StatusCode::FORBIDDEN);
    }
    Ok(parsed)
}

async fn create(
    State(state): State<StateAuthorImage>,
    Path(book): Path<i64>,
    headers: HeaderMap,
    parts: Multipart,
) -> Response {
    if !state.cookies.same_origin(&headers) {
        return deny(StatusCode::FORBIDDEN);
    }
    let actor = match owner(&state, &headers, book).await {
        Ok(Some(actor)) => actor,
        Ok(None) => return deny(StatusCode::UNAUTHORIZED),
        Err(code) => return deny(code),
    };
    let Ok(_permit) = state.pending.clone().try_acquire_owned() else {
        return deny(StatusCode::TOO_MANY_REQUESTS);
    };
    let mut form = match read_form(parts, &headers, &state.cookies).await {
        Ok(form) => form,
        Err(code) => return deny(code),
    };
    // RIL-08: repeat the session-bound anti-CSRF assertion at the handler
    // boundary, so a future parser change cannot silently relax it.
    if !state
        .cookies
        .valid_csrf(&headers, form.csrf.as_deref().unwrap_or(""))
    {
        return deny(StatusCode::FORBIDDEN);
    }
    let Some((mime, bytes)) = form.image.take() else {
        return deny(StatusCode::BAD_REQUEST);
    };
    // NEwBOO image safety: dimensions, metadata and animation restrictions.
    // Bound memory/CPU before invoking expensive G encryption and S3.
    let check_bytes = bytes.clone();
    let check_mime = mime.clone();
    if !matches!(
        tokio::task::spawn_blocking(move || validate_cover_source(&check_bytes, &check_mime)).await,
        Ok(Ok(()))
    ) {
        return deny(StatusCode::UNSUPPORTED_MEDIA_TYPE);
    }
    let Some(command) = form.command else {
        return deny(StatusCode::BAD_REQUEST);
    };
    let image_id = stable_image_id(actor, book, &command);
    let Some(body) = form.body else {
        return deny(StatusCode::BAD_REQUEST);
    };
    // This intentionally appends ONE image. Arbitrary in-text positioning
    // requires a later staged editor with safe stable placeholder handling.
    // The B edition and G AAD both see this exact immutable final Markdown.
    if body.contains("asset:") {
        return deny(StatusCode::BAD_REQUEST);
    }
    let body = format!(
        "{}\n\n![Иллюстрация](asset:{})",
        body.replace("\r\n", "\n").replace('\r', "\n"),
        image_id.as_str()
    );
    let input = match save_chapter(SaveChapterRequest {
        book_id: match u64::try_from(book) {
            Ok(book) if book > 0 => book,
            _ => return deny(StatusCode::NOT_FOUND),
        },
        chapter_id: None,
        number: form.number.unwrap_or(0),
        expected_revision: 0,
        title: form.title.as_deref().unwrap_or(""),
        body: &body,
        access: "paid",
        price_amount: form.price.unwrap_or(0),
        command_key: &command,
    }) {
        Ok(value) => value,
        Err(_) => return deny(StatusCode::BAD_REQUEST),
    };
    match state
        .publisher
        .publish_with_image(actor, &input, &image_id, &bytes, &mime)
        .await
    {
        Ok(()) => private_response(Redirect::to(&format!("/books/{book}")), &[]),
        Err(AuthorError::Missing) => deny(StatusCode::NOT_FOUND),
        Err(AuthorError::Invalid) => deny(StatusCode::BAD_REQUEST),
        Err(AuthorError::Forbidden) => deny(StatusCode::FORBIDDEN),
        Err(AuthorError::Conflict) => deny(StatusCode::CONFLICT),
        Err(AuthorError::Unavailable) => deny(StatusCode::SERVICE_UNAVAILABLE),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn same_author_command_mints_one_version_bound_marker_on_retry() {
        let key = tor_accounts::services::security::generate_session_token();
        let first = stable_image_id(7, 42, &key);
        assert_eq!(first, stable_image_id(7, 42, &key));
        assert_ne!(first, stable_image_id(8, 42, &key));
        assert_ne!(first, stable_image_id(7, 43, &key));
        assert_eq!(first.as_str().len(), 36);
        assert_eq!(&first.as_str()[14..15], "4");
        assert!(matches!(&first.as_str()[19..20], "8" | "9" | "a" | "b"));
    }
}
