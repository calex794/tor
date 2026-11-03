//! S065/C-12: separate, no-purchase site-administrator reader.
//! No keys, private bytes, body or chosen version appear in this HTML.
use super::*;
use axum::response::{Html, IntoResponse};

pub(super) async fn admin_reader(
    State(state): State<GrantState>,
    Path(raw_chapter): Path<String>,
    headers: HeaderMap,
) -> Response {
    let s3_origin = state.reader_s3_origin.as_deref();
    let Some(session) = state.cookies.session_token(&headers) else {
        return deny(StatusCode::UNAUTHORIZED);
    };
    let digest = session_token_digest(&session);
    let actor = match state.accounts.session_actor(&digest).await {
        Ok(Some(actor)) if actor.administrator => actor,
        Ok(Some(_)) => return deny(StatusCode::FORBIDDEN),
        Ok(None) => return deny(StatusCode::UNAUTHORIZED),
        Err(_) => return deny(StatusCode::SERVICE_UNAVAILABLE),
    };
    let Some(chapter_id) = positive(&raw_chapter) else {
        return deny(StatusCode::NOT_FOUND);
    };
    let Ok(actor_id) = PositiveId::new(actor.actor.user_id.get()) else {
        return deny(StatusCode::SERVICE_UNAVAILABLE);
    };
    let authority = PgAdministratorReaderEntitlements::new(state.accounts.pool().clone(), digest);
    // A current unsealed working draft takes precedence over a previously
    // sealed publication; its text is requested separately with live CSRF.
    // Do not fall back to stale sealed content when a draft exists but fails.
    let unsealed = match authority
        .unsealed_version(Actor { user_id: actor_id }, chapter_id)
        .await
    {
        Ok(Some(_)) => true,
        Ok(None) => false,
        Err(error) => return deny(status(error)),
    };
    if !unsealed {
        // A sealed chapter still requires a configured S3 browser. Do not
        // offer a fake encrypted reader when the private origin is absent.
        if s3_origin.is_none() {
            return deny(StatusCode::NOT_FOUND);
        }
        match authority
            .resolve_entitlement(Actor { user_id: actor_id }, chapter_id, None)
            .await
        {
            Ok(EntitlementDecision::Allowed { pinned }) if pinned.chapter_id == chapter_id => {}
            Ok(EntitlementDecision::Allowed { .. }) => return deny(StatusCode::CONFLICT),
            Ok(EntitlementDecision::Denied { .. }) => return deny(StatusCode::FORBIDDEN),
            Err(error) => return deny(status(error)),
        }
    }
    let (csrf, cookies) = state.cookies.csrf_for_page(&headers);
    let s3_origin = s3_origin.unwrap_or("");
    let content = format!(
        r#"<section class="section reader-page reader-private reader-paid"
                     data-admin-chapter-reader data-chapter-id="{chapter}"
                     data-admin-unsealed="{unsealed}"
                     data-csrf-token="{csrf}" data-s3-origin="{origin}"
                     aria-labelledby="admin-reader-title">
             <header class="reader-topbar">
               <a class="reader-topbar__back" href="/me/account">← Личный кабинет</a>
               <div class="reader-topbar__title">
                 <span class="reader-topbar__book">Административный просмотр</span>
                 <span class="reader-topbar__chapter">Глава {chapter}</span>
               </div>
             </header>
             <div class="container reader-width">
               <div class="reader-page__header">
                 <p class="reader-page__meta">Чтение без покупки — по действующей роли администратора</p>
                 <h1 class="reader-page__title" id="admin-reader-title">Глава {chapter}</h1>
                 <p class="reader-page__subtitle" data-admin-status role="status">
                   Проверка полномочий…
                 </p>
               </div>
               <article class="chapter-content reader-markdown reader-paid__content"
                        data-admin-content aria-label="Административное чтение главы"></article>
             </div>
             <script type="module" src="/assets/newboo-admin-reader.mjs"></script>
           </section>"#,
        chapter = chapter_id.get(),
        unsealed = unsealed,
        csrf = crate::pages::escape_html(&csrf),
        origin = crate::pages::escape_html(s3_origin),
    );
    let html = include_str!("../../templates/reader_layout.html")
        .replace(
            "<!--PAGE_TITLE-->",
            &format!("Глава {} — Администратор", chapter_id.get()),
        )
        .replace(
            "<!--PAGE_DESCRIPTION-->",
            "Приватный административный просмотр главы.",
        )
        .replace("<!--PAGE_CONTENT-->", &content);
    let mut response = private_response(Html(html), &cookies);
    let optional_s3 = if s3_origin.is_empty() {
        String::new()
    } else {
        format!(" {s3_origin}")
    };
    let csp = format!(
        "default-src 'none'; script-src 'self'; style-src 'self'; img-src 'self' blob:; \
         connect-src 'self'{optional_s3}; form-action 'self'; base-uri 'none'; frame-ancestors 'none'"
    );
    let Ok(value) = HeaderValue::from_str(&csp) else {
        return deny(StatusCode::SERVICE_UNAVAILABLE);
    };
    response
        .headers_mut()
        .insert("content-security-policy", value);
    response.headers_mut().insert(
        header::REFERRER_POLICY,
        HeaderValue::from_static("no-referrer"),
    );
    response
        .headers_mut()
        .insert("x-robots-tag", HeaderValue::from_static("noindex"));
    response
}

pub(super) async fn script() -> impl IntoResponse {
    (
        [
            (
                header::CONTENT_TYPE,
                "application/javascript; charset=utf-8",
            ),
            (header::CACHE_CONTROL, "public, max-age=86400"),
        ],
        include_str!("../../assets/newboo-admin-reader.mjs"),
    )
}
