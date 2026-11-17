//! D-05 / BR-060: actor-owned historical Layer FREE SINGLE edition.
//! Never use the public /reader or /free-grant routes to bypass Book deindexing.
//! The immutable archive identity and history originate in D + B PostgreSQL.
use super::{private_response, session_token_digest, sign, verified_grant, FreeGrant, FreeGrantState};
use axum::{
    extract::{Path, State},
    http::{header, HeaderMap, HeaderValue, StatusCode},
    response::{Html, IntoResponse, Response},
    Json,
};
use sqlx::query_as;
use tor_contracts::paid_delivery::PositiveId;
use tor_reading::repositories::free_s3_ref::FreeS3Ref;

fn deny(code: StatusCode) -> Response {
    private_response(code, &[])
}

fn csrf_valid(cookies: &super::CookiePolicy, headers: &HeaderMap) -> bool {
    headers.get(header::ORIGIN).and_then(|v| v.to_str().ok())
        == Some(cookies.origin.as_str())
        && cookies.same_origin(headers)
        && headers
            .get("x-csrf-token")
            .and_then(|v| v.to_str().ok())
            .is_some_and(|token| cookies.valid_csrf(headers, token))
}

async fn actor(
    state: &FreeGrantState,
    headers: &HeaderMap,
) -> Result<PositiveId, StatusCode> {
    let session = state
        .cookies
        .session_token(headers)
        .ok_or(StatusCode::UNAUTHORIZED)?;
    let id = state
        .accounts
        .active_session_user(&session_token_digest(&session))
        .await
        .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?
        .ok_or(StatusCode::UNAUTHORIZED)?;
    PositiveId::new(id).map_err(|_| StatusCode::SERVICE_UNAVAILABLE)
}

/// Signed SINGLE is only possible for a verified frozen edition backed by
/// precisely the author-written S3 object. Neither the browser nor a
/// storefront version selects the object, bucket or publication version.
async fn authorized_free_source(
    actor: PositiveId,
    state: &FreeGrantState,
    chapter: PositiveId,
) -> Result<Option<FreeS3Ref>, StatusCode> {
    let Some(version) = state
        .layer
        .archived_free_version(actor, chapter)
        .await
        .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?
    else {
        return Ok(None);
    };
    let row: Option<(i64, i64, Vec<u8>, i64, String)> = query_as(
        "SELECT f.chapter_id,f.version_id,f.sha256,f.size_bytes,f.bucket
         FROM free_chapter_storage_objects f
         JOIN chapter_versions v ON v.id=f.version_id
           AND v.chapter_id=f.chapter_id
           AND v.content_sha256=f.sha256
         WHERE f.chapter_id=$1 AND f.version_id=$2",
    )
    .bind(chapter.get())
    .bind(version.get())
    .fetch_optional(state.accounts.pool())
    .await
    .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?;
    let Some((chapter_id, version_id, sha, size, bucket)) = row else {
        // A verified active PACK edition needs its own bounded range gateway.
        // Never invent a SINGLE receipt or sign a whole PACK as a fallback.
        return Ok(None);
    };
    let sha256: [u8; 32] = sha
        .try_into()
        .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?;
    let size_bytes = u64::try_from(size).map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?;
    if size_bytes == 0 || size_bytes > 200_000 || bucket != state.bucket {
        return Err(StatusCode::SERVICE_UNAVAILABLE);
    }
    Ok(Some(FreeS3Ref {
        chapter_id,
        version_id,
        sha256,
        size_bytes,
        bucket,
    }))
}

fn shell(chapter: i64, csrf: &str, origin: &str) -> Html<String> {
    let content = format!(
        r#"<section class="section reader-page reader-private"
          data-layer-free-reader data-chapter-id="{chapter}"
          data-csrf-token="{csrf}" data-s3-origin="{origin}">
          <header class="reader-topbar" aria-label="Архивное чтение Layer">
            <a class="reader-topbar__back" href="/me/library/layers">К моим пакетам</a>
            <span class="reader-topbar__chapter">Архивная бесплатная глава</span>
          </header>
          <div class="container reader-width">
            <p class="reader-page__meta" data-layer-archive="1">
              Удалена с сайта · доступна вам в архиве Layer
            </p>
            <h1 class="reader-page__title">Бесплатная глава</h1>
            <p data-layer-free-status role="status">Проверяем действующую подписку…</p>
            <article class="chapter-content reader-markdown reader-paid__content"
              data-layer-free-content aria-label="Архивный текст главы"></article>
            <p class="reader-page__return">
              <a href="/me/library/layers">К моим пакетам</a>
            </p>
          </div>
          <script type="module" src="/assets/newboo-layer-free-reader.mjs"></script>
        </section>"#,
        csrf = crate::pages::escape_html(csrf),
        origin = crate::pages::escape_html(origin),
    );
    Html(
        include_str!("../../templates/reader_layout.html")
            .replace("<!--PAGE_TITLE-->", "Архивная бесплатная глава — Layer")
            .replace(
                "<!--PAGE_DESCRIPTION-->",
                "Защищённое чтение ранее опубликованной бесплатной редакции.",
            )
            .replace("<!--PAGE_CONTENT-->", &content),
    )
}

/// GET contains no object URL, version, SHA, body or secret. D is checked
/// before even the generic private shell can be enumerated.
pub(super) async fn page(
    State(state): State<FreeGrantState>,
    Path(id): Path<i64>,
    headers: HeaderMap,
) -> Response {
    let Ok(chapter) = PositiveId::new(id) else {
        return deny(StatusCode::NOT_FOUND);
    };
    let actor = match actor(&state, &headers).await {
        Ok(actor) => actor,
        Err(status) => return deny(status),
    };
    match authorized_free_source(actor, &state, chapter).await {
        Ok(Some(_)) => {}
        Ok(None) => return deny(StatusCode::NOT_FOUND),
        Err(status) => return deny(status),
    }
    let (csrf, cookies) = state.cookies.csrf_for_page(&headers);
    let mut response = private_response(shell(id, &csrf, &state.s3_origin), &cookies);
    let csp = format!(
        "default-src 'none'; script-src 'self'; style-src 'self'; img-src 'self' blob:; \
         connect-src 'self' {}; form-action 'self'; base-uri 'none'; frame-ancestors 'none'",
        state.s3_origin,
    );
    let Ok(value) = HeaderValue::from_str(&csp) else {
        return deny(StatusCode::SERVICE_UNAVAILABLE);
    };
    response.headers_mut().insert("content-security-policy", value);
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

/// POST requires a real same-origin browser Origin and double-submit CSRF.
/// It reauthorizes actor, D term and exact S3 identity AFTER slow signing.
pub(super) async fn grant(
    State(state): State<FreeGrantState>,
    Path(id): Path<i64>,
    headers: HeaderMap,
) -> Response {
    let actor = match actor(&state, &headers).await {
        Ok(actor) => actor,
        Err(status) => return deny(status),
    };
    if !csrf_valid(&state.cookies, &headers) {
        return deny(StatusCode::FORBIDDEN);
    }
    let Ok(chapter) = PositiveId::new(id) else {
        return deny(StatusCode::NOT_FOUND);
    };
    let result = verified_grant::issue(
        || authorized_free_source(actor, &state, chapter),
        |record| sign(&state, record),
    )
    .await;
    let (record, signed) = match result {
        Ok(grant) => grant,
        Err(status) => return deny(status),
    };
    let mut response = private_response(
        Json(FreeGrant {
            schema: "tor.layer-free-s3-grant.v1",
            chapter_id: record.chapter_id.to_string(),
            version_id: record.version_id.to_string(),
            url: signed,
            sha256: record
                .sha256
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect(),
            size_bytes: record.size_bytes,
        }),
        &[],
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

pub(super) async fn script() -> impl IntoResponse {
    (
        [
            (header::CONTENT_TYPE, "application/javascript; charset=utf-8"),
            (header::CACHE_CONTROL, "public, max-age=3600"),
        ],
        include_str!("../../assets/newboo-layer-free-reader.mjs"),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn private_shell_escapes_data_and_never_contains_a_storage_capability() {
        let html = shell(42, "fake<script>", "https://bucket.example.test").0;
        assert!(html.contains("data-layer-free-reader"));
        assert!(html.contains("data-chapter-id=\"42\""));
        assert!(html.contains("fake&lt;script&gt;"));
        assert!(!html.contains("fake<script>"));
        assert!(!html.contains("X-Amz-Signature"));
        assert!(!html.contains("purchase_id"));
        assert!(!html.contains("dek_b64u"));
        assert!(html.contains("Удалена с сайта"));
    }

    #[test]
    fn signed_layer_free_grant_requires_real_origin_and_double_submit_csrf() {
        let cookies = super::super::CookiePolicy {
            secure: false,
            origin: "https://tor.example.test".to_owned(),
        };
        let token = "A".repeat(43);
        let mut headers = HeaderMap::new();
        headers.insert(header::ORIGIN, "https://tor.example.test".parse().unwrap());
        headers.insert("x-csrf-token", token.parse().unwrap());
        headers.insert(
            header::COOKIE,
            format!("tor-dev-csrf={token}").parse().unwrap(),
        );
        assert!(csrf_valid(&cookies, &headers));

        headers.insert(header::ORIGIN, "https://foreign.example.test".parse().unwrap());
        assert!(!csrf_valid(&cookies, &headers));
        headers.insert(header::ORIGIN, "https://tor.example.test".parse().unwrap());
        headers.insert("x-csrf-token", "B".repeat(43).parse().unwrap());
        assert!(!csrf_valid(&cookies, &headers));
        headers.insert("x-csrf-token", token.parse().unwrap());
        headers.insert("sec-fetch-site", "cross-site".parse().unwrap());
        assert!(!csrf_valid(&cookies, &headers));
        headers.remove("sec-fetch-site");
        headers.remove(header::ORIGIN);
        assert!(!csrf_valid(&cookies, &headers));
    }
}
