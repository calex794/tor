//! Session-aware navigation for server-rendered pages.
//! The session is checked only on HTML-like GET routes, and never grants access.
use axum::{
    extract::{Request, State},
    http::{header, HeaderValue, Method},
    middleware::Next,
    response::Response,
    Router,
};
use tor_accounts::{repositories::PgAccounts, services::security::session_token_digest};

use crate::auth_cookies::CookiePolicy;

#[derive(Clone, Default)]
struct Visitor {
    authenticated: bool,
    csrf: Option<String>,
}

tokio::task_local! {
    static VISITOR: Visitor;
}

#[derive(Clone)]
struct NavigationState {
    accounts: PgAccounts,
    cookies: CookiePolicy,
}

pub(crate) fn instrument(app: Router, accounts: PgAccounts, cookies: CookiePolicy) -> Router {
    app.layer(axum::middleware::from_fn_with_state(
        NavigationState { accounts, cookies },
        with_visitor,
    ))
}

fn html_route(method: &Method, path: &str) -> bool {
    if method != &Method::GET {
        return false;
    }
    ![
        "/assets/",
        "/api/",
        "/covers/",
        "/sitemaps/",
        "/favicon",
        "/icon-",
        "/apple-touch-icon",
        "/ops/",
    ]
    .iter()
    .any(|prefix| path.starts_with(prefix))
        && ![
            "/healthz",
            "/readyz",
            "/metrics",
            "/robots.txt",
            "/sitemap.xml",
            "/site.webmanifest",
        ]
        .contains(&path)
}

async fn with_visitor(
    State(state): State<NavigationState>,
    request: Request,
    next: Next,
) -> Response {
    if !html_route(request.method(), request.uri().path()) {
        return next.run(request).await;
    }

    let session = state.cookies.session_token(request.headers());
    let authenticated = if let Some(token) = session.as_deref() {
        matches!(
            state
                .accounts
                .active_session_user(&session_token_digest(token))
                .await,
            Ok(Some(_))
        )
    } else {
        false
    };
    // Only a verified session may receive a working logout form.
    let (csrf, csrf_cookies) = if authenticated {
        let (token, cookies) = state.cookies.csrf_for_page(request.headers());
        (Some(token), cookies)
    } else {
        (None, Vec::new())
    };
    let visitor = Visitor {
        authenticated,
        csrf,
    };
    let mut response = VISITOR.scope(visitor, next.run(request)).await;
    let is_html = response
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value.starts_with("text/html"));
    if is_html {
        // Personalized pages must never be shared through a public HTML cache.
        response
            .headers_mut()
            .append(header::VARY, HeaderValue::from_static("Cookie"));
        if session.is_some() {
            response.headers_mut().insert(
                header::CACHE_CONTROL,
                HeaderValue::from_static("private, no-store"),
            );
        }
        for cookie in csrf_cookies {
            if let Ok(mut value) = HeaderValue::from_str(&cookie) {
                value.set_sensitive(true);
                response.headers_mut().append(header::SET_COOKIE, value);
            }
        }
    }
    response
}

/// Public/demo renders have no verified session context and always show guest links.
pub(crate) fn render(html: String) -> String {
    let visitor = VISITOR.try_with(Clone::clone).unwrap_or_default();
    render_for(&html, &visitor)
}

fn render_for(html: &str, visitor: &Visitor) -> String {
    let csrf = crate::pages::escape_html(visitor.csrf.as_deref().unwrap_or(""));
    let (mobile, desktop) = if visitor.authenticated {
        (
            format!(
                r#"<a class="site-mobile-nav__link" href="/me">Кабинет</a>
<a class="site-mobile-nav__link" href="/me/books">Мои книги</a>
<form class="site-header__logout" method="post" action="/logout">
<input type="hidden" name="csrf_token" value="{csrf}">
<button class="site-mobile-nav__link" type="submit">Выйти</button>
</form>"#
            ),
            format!(
                r#"<a class="site-nav__link" href="/me/books">Мои книги</a>
<a class="button button--small" href="/me">Кабинет</a>
<form class="site-header__logout" method="post" action="/logout">
<input type="hidden" name="csrf_token" value="{csrf}">
<button class="site-nav__link" type="submit">Выйти</button>
</form>"#
            ),
        )
    } else {
        (
            r#"<a class="site-mobile-nav__link" href="/login">Войти</a>
<a class="site-mobile-nav__link" href="/register">Регистрация</a>"#
                .to_owned(),
            r#"<a class="site-nav__link" href="/login">Войти</a>
<a class="button button--small" href="/register">Регистрация</a>"#
                .to_owned(),
        )
    };
    html.replace("<!--HEADER_MOBILE_AUTH-->", &mobile)
        .replace("<!--HEADER_DESKTOP_AUTH-->", &desktop)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn navigation_is_scoped_to_html_gets() {
        assert!(html_route(&Method::GET, "/me/books/2"));
        assert!(html_route(&Method::GET, "/catalog"));
        assert!(!html_route(&Method::POST, "/me/books"));
        assert!(!html_route(&Method::GET, "/api/v1/books"));
        assert!(!html_route(&Method::GET, "/assets/site.css"));
    }

    #[test]
    fn guest_and_verified_headers_are_mutually_exclusive() {
        let template = include_str!("../templates/layout.html");
        let guest = render_for(template, &Visitor::default());
        assert!(guest.contains("href=\"/login\">Войти</a>"));
        assert!(guest.contains("href=\"/register\">Регистрация</a>"));
        assert!(!guest.contains("action=\"/logout\""));
        // The creation CTA remains visible; its route enforces the session.
        assert!(guest.contains("href=\"/me/books/new\""));
        let signed_in = render_for(
            template,
            &Visitor {
                authenticated: true,
                csrf: Some("csrf-token".to_owned()),
            },
        );
        assert!(!signed_in.contains("href=\"/login\">Войти</a>"));
        assert!(!signed_in.contains("href=\"/register\">Регистрация</a>"));
        assert!(signed_in.contains("href=\"/me/books\">Мои книги</a>"));
        assert!(signed_in.contains("name=\"csrf_token\" value=\"csrf-token\""));
        assert!(signed_in.contains("action=\"/logout\""));
    }
}
