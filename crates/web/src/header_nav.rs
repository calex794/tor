//! Session-aware navigation for server-rendered pages.
//! The session is checked only on HTML-like GET routes, and never grants access.
use axum::{
    extract::{Request, State},
    http::{header, HeaderMap, HeaderValue, Method, StatusCode},
    middleware::Next,
    response::Response,
    Router,
};
use base64::{engine::general_purpose::STANDARD, Engine as _};
use sha2::{Digest, Sha256};
use tor_accounts::{repositories::PgAccounts, services::security::session_token_digest};

use crate::auth_cookies::CookiePolicy;

#[derive(Clone, Default)]
struct Visitor {
    authenticated: bool,
    csrf: Option<String>,
    auth_dialog: bool,
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
    if method != Method::GET {
        return false;
    }
    // Login and registration always issue their own form CSRF cookie.
    if path == "/login" || path == "/register" {
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

// Only a user-activated same-origin document navigation counts here, not
// background fetches, automatic reloads, prefetch, images or health probes.
fn user_navigation(headers: &HeaderMap) -> bool {
    let value = |name| headers.get(name).and_then(|v| v.to_str().ok());
    matches!(value("sec-fetch-site"), Some("same-origin" | "none"))
        && value("sec-fetch-mode") == Some("navigate")
        && value("sec-fetch-dest") == Some("document")
        && value("sec-fetch-user") == Some("?1")
}

// Only public browsing pages can open the progressive-enhancement dialog.
// Private account, admin and standalone authentication pages keep their CSP.
fn auth_dialog_route(path: &str) -> bool {
    matches!(path, "/" | "/catalog")
        || ["/books/", "/reader/"].into_iter().any(|prefix| {
            path.strip_prefix(prefix)
                .is_some_and(|id| !id.is_empty() && id.bytes().all(|byte| byte.is_ascii_digit()))
        })
}

// Bind the script's integrity to the exact bytes served by this executable.
// The hash in the URL also prevents a previously cached version being reused.
pub(crate) fn dialog_integrity() -> &'static str {
    static INTEGRITY: std::sync::LazyLock<String> = std::sync::LazyLock::new(|| {
        let bytes = include_bytes!("../assets/tor-auth-dialog.js");
        format!("sha256-{}", STANDARD.encode(Sha256::digest(bytes)))
    });
    INTEGRITY.as_str()
}

/// Public guest book and paywall documents otherwise use the strict, script-free
/// policy. Allow only the exact SRI-pinned dialog, never general inline scripts,
/// arbitrary same-origin scripts or third-party connections. The caller checks
/// the guest route, HTML content type and successful status before applying it.
fn allow_pinned_dialog(response: &mut Response) {
    let name = "content-security-policy";
    if response.headers().get_all(name).iter().count() != 1 {
        return;
    }
    let Some(policy) = response.headers().get(name).and_then(|v| v.to_str().ok()) else {
        return;
    };
    if !policy.contains("script-src 'none';") || policy.contains("connect-src") {
        return;
    }
    let updated = policy.replacen(
        "script-src 'none';",
        &format!("script-src '{}'; connect-src 'self';", dialog_integrity()),
        1,
    );
    if let Ok(value) = HeaderValue::from_str(&updated) {
        response.headers_mut().insert(name, value);
    }
}

// Read-only administrative reports must neither issue CSRF cookies nor expose
// a preexisting CSRF token in HTML. Logout remains available from /me.
fn may_issue_header_csrf(path: &str) -> bool {
    path != "/odmen" && !path.starts_with("/odmen/")
}

/// Make a freshly minted CSRF cookie visible to both the header and the
/// downstream page form. Mutating routes never pass through this GET-only helper.
fn install_page_csrf(headers: &mut HeaderMap, name: &str, token: &str) -> Result<(), StatusCode> {
    let mut cookies = Vec::new();
    for value in headers.get_all(header::COOKIE) {
        let value = value.to_str().map_err(|_| StatusCode::BAD_REQUEST)?;
        for item in value.split(';') {
            let item = item.trim();
            if item.is_empty()
                || item
                    .split_once('=')
                    .is_some_and(|(key, _)| key.trim() == name)
            {
                continue;
            }
            cookies.push(item);
        }
    }
    cookies.push(token);
    // Preserve the original cookies; replace only the invalid/missing CSRF pair.
    let mut pairs = cookies[..cookies.len() - 1].join("; ");
    if !pairs.is_empty() {
        pairs.push_str("; ");
    }
    pairs.push_str(name);
    pairs.push('=');
    pairs.push_str(token);
    let value = HeaderValue::from_str(&pairs).map_err(|_| StatusCode::BAD_REQUEST)?;
    headers.insert(header::COOKIE, value);
    Ok(())
}

async fn with_visitor(
    State(state): State<NavigationState>,
    mut request: Request,
    next: Next,
) -> Response {
    if !html_route(request.method(), request.uri().path()) {
        return next.run(request).await;
    }

    let session = state.cookies.session_token(request.headers());
    let renewal_navigation = may_issue_header_csrf(request.uri().path())
        && state.cookies.same_origin(request.headers())
        && user_navigation(request.headers());
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
        if may_issue_header_csrf(request.uri().path()) {
            let (token, cookies) = state.cookies.csrf_for_page(request.headers());
            if !cookies.is_empty() {
                if let Err(status) =
                    install_page_csrf(request.headers_mut(), state.cookies.csrf_name(), &token)
                {
                    return Response::builder()
                        .status(status)
                        .body(axum::body::Body::empty())
                        .expect("valid status");
                }
            }
            (Some(token), cookies)
        } else {
            // Admin report views stay token-free even when the browser already
            // has a CSRF cookie. Logout remains reachable from /me.
            (None, Vec::new())
        }
    } else {
        (None, Vec::new())
    };
    let auth_dialog = !authenticated && auth_dialog_route(request.uri().path());
    let visitor = Visitor {
        authenticated,
        csrf,
        auth_dialog,
    };
    let mut response = VISITOR.scope(visitor, next.run(request)).await;
    let is_html = response
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value.starts_with("text/html"));
    if is_html {
        if auth_dialog && response.status() == StatusCode::OK {
            allow_pinned_dialog(&mut response);
        }
        // Personalized pages must never be shared through a public HTML cache.
        response
            .headers_mut()
            .append(header::VARY, HeaderValue::from_static("Cookie"));
        if session.is_some() && !response.headers().contains_key(header::CACHE_CONTROL) {
            response
                .headers_mut()
                .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
        }
        for cookie in csrf_cookies {
            if let Ok(mut value) = HeaderValue::from_str(&cookie) {
                value.set_sensitive(true);
                response.headers_mut().append(header::SET_COOKIE, value);
            }
        }
    }
    // Keep administrative report responses cookie-free. Do not replace an
    // explicit login/logout/rotation cookie from the handler.
    if is_html && response.status() == StatusCode::OK && authenticated && renewal_navigation {
        if let Some(token) = session.as_deref() {
            let prefix = format!("{}=", state.cookies.session_name());
            let already_set = response
                .headers()
                .get_all(header::SET_COOKIE)
                .iter()
                .any(|v| v.to_str().is_ok_and(|v| v.starts_with(&prefix)));
            if !already_set {
                if let Some(cookie) = state.cookies.activity_cookie(&state.accounts, token).await {
                    if let Ok(mut value) = HeaderValue::from_str(&cookie) {
                        value.set_sensitive(true);
                        response.headers_mut().append(header::SET_COOKIE, value);
                        response
                            .headers_mut()
                            .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
                    }
                }
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
    let (mobile, desktop, member_nav) = if visitor.authenticated {
        let (mobile_logout, desktop_logout) = visitor
            .csrf
            .as_deref()
            .map(crate::pages::escape_html)
            .map(|csrf| {
                (
                    format!(
                        r#"<form class="site-header__logout" method="post" action="/logout">
<input type="hidden" name="csrf_token" value="{csrf}">
<button class="site-mobile-nav__link" type="submit">Выйти</button>
</form>"#
                    ),
                    format!(
                        r#"<form class="site-header__logout" method="post" action="/logout">
<input type="hidden" name="csrf_token" value="{csrf}">
<button class="site-nav__link" type="submit">Выйти</button>
</form>"#
                    ),
                )
            })
            .unwrap_or_default();
        (
            format!(
                r#"<a class="site-mobile-nav__link" href="/me/library">Библиотека</a>
<a class="site-mobile-nav__link site-mobile-nav__link--create" href="/me/books/new">Создать книгу</a>
<a class="site-mobile-nav__link" href="/me">Кабинет</a>
<a class="site-mobile-nav__link" href="/me/books">Мои книги</a>
{mobile_logout}"#
            ),
            format!(
                r#"<a class="site-nav__link" href="/me/books">Мои книги</a>
<a class="button button--small" href="/me">Кабинет</a>
{desktop_logout}"#
            ),
            r#"<a class="site-nav__link" href="/me/library">Библиотека</a>
<a class="button button--small site-nav__create" href="/me/books/new">Создать книгу</a>"#,
        )
    } else {
        (
            r#"<a class="site-mobile-nav__link" href="/login">Войти</a>"#.to_owned(),
            r#"<a class="button button--small" href="/login">Войти</a>"#.to_owned(),
            "",
        )
    };
    let dialog = if visitor.auth_dialog && !visitor.authenticated {
        include_str!("../templates/auth_dialog.html")
            .replace("<!--AUTH_DIALOG_INTEGRITY-->", dialog_integrity())
    } else {
        String::new()
    };
    html.replace("<!--HEADER_MOBILE_AUTH-->", &mobile)
        .replace("<!--HEADER_DESKTOP_MEMBER_NAV-->", member_nav)
        .replace("<!--HEADER_DESKTOP_AUTH-->", &desktop)
        .replace("<!--HEADER_AUTH_DIALOG-->", &dialog)
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
    fn admin_reports_remain_cookie_free_without_a_preexisting_csrf_token() {
        assert!(may_issue_header_csrf("/me/books/new"));
        assert!(!may_issue_header_csrf("/odmen"));
        assert!(!may_issue_header_csrf("/odmen/audience/reading"));
        assert!(!may_issue_header_csrf("/odmen/mail/book-rooms"));
        let html = render_for(
            include_str!("../templates/layout.html"),
            &Visitor {
                authenticated: true,
                csrf: None,
                auth_dialog: false,
            },
        );
        assert!(html.contains("href=\"/me/books\">Мои книги</a>"));
        assert!(!html.contains("action=\"/logout\""));
        assert!(!html.contains("href=\"/login\">Войти</a>"));
    }

    #[test]
    fn csrf_renewal_uses_exactly_one_token_in_header_and_form() {
        let policy = CookiePolicy {
            secure: false,
            origin: "https://tor.example".to_owned(),
        };
        let mut headers = HeaderMap::new();
        headers.insert(
            header::COOKIE,
            HeaderValue::from_static("tor-dev-session=abc; tor-dev-csrf=invalid"),
        );
        assert!(!policy.valid_csrf(&headers, "invalid"));
        let (token, issued) = policy.csrf_for_page(&headers);
        assert_eq!(issued.len(), 1);
        install_page_csrf(&mut headers, policy.csrf_name(), &token).unwrap();
        let (downstream, extra) = policy.csrf_for_page(&headers);
        assert_eq!(downstream, token);
        assert!(extra.is_empty());
        assert!(policy.valid_csrf(&headers, &token));
    }

    #[test]
    fn dialog_is_guest_public_only() {
        for path in ["/", "/catalog", "/books/1", "/reader/101"] {
            assert!(auth_dialog_route(path), "{path}");
        }
        for path in ["/login", "/register", "/me", "/odmen", "/read/101"] {
            assert!(!auth_dialog_route(path), "{path}");
        }
        let template = include_str!("../templates/layout.html");
        let guest = render_for(
            template,
            &Visitor {
                auth_dialog: true,
                ..Visitor::default()
            },
        );
        assert_eq!(guest.matches("id=\"tor-auth-dialog\"").count(), 1);
        assert!(guest.contains("/assets/tor-auth-dialog.js"));
        assert!(guest.contains(&format!("integrity=\"{}\"", dialog_integrity())));
        assert!(!guest.contains("<!--AUTH_DIALOG_INTEGRITY-->"));
        let signed_in = render_for(
            template,
            &Visitor {
                authenticated: true,
                auth_dialog: true,
                ..Visitor::default()
            },
        );
        assert!(!signed_in.contains("id=\"tor-auth-dialog\""));
        assert!(!signed_in.contains("/assets/tor-auth-dialog.js"));
    }

    #[test]
    fn strict_guest_policy_allows_only_the_integrity_pinned_dialog() {
        let mut reply = crate::auth_cookies::private_response("guest", &[]);
        let before = reply.headers()["content-security-policy"].clone();
        allow_pinned_dialog(&mut reply);
        let policy = reply.headers()["content-security-policy"].to_str().unwrap();
        let expected = before.to_str().unwrap().replacen(
            "script-src 'none';",
            &format!("script-src '{}'; connect-src 'self';", dialog_integrity()),
            1,
        );
        assert_eq!(policy, expected);
        assert!(!policy.contains("script-src 'self'"));
        assert!(!policy.contains("unsafe-inline"));
        assert!(!policy.contains("unsafe-eval"));
        assert_eq!(reply.headers()[header::CACHE_CONTROL], "no-store");
        let pinned = reply.headers()["content-security-policy"].clone();
        allow_pinned_dialog(&mut reply);
        assert_eq!(reply.headers()["content-security-policy"], pinned);
        let mut free = crate::auth_cookies::private_response("free", &[]);
        let existing = HeaderValue::from_static("script-src 'self'; connect-src 'self';");
        free.headers_mut()
            .insert("content-security-policy", existing.clone());
        allow_pinned_dialog(&mut free);
        assert_eq!(free.headers()["content-security-policy"], existing);
    }

    #[test]
    fn guest_and_verified_headers_are_mutually_exclusive() {
        let template = include_str!("../templates/layout.html");
        let guest = render_for(template, &Visitor::default());
        assert!(guest.contains("href=\"/login\">Войти</a>"));
        assert!(!guest.contains("href=\"/register\">Регистрация</a>"));
        assert!(!guest.contains("action=\"/logout\""));
        let guest_header = guest.split("</header>").next().unwrap();
        assert!(!guest_header.contains("href=\"/me/books/new\""));
        assert!(!guest_header.contains("href=\"/me/library\""));
        assert!(!guest_header.contains("href=\"/me\""));
        assert!(!guest_header.contains("href=\"/me/books\""));
        assert_eq!(guest_header.matches("href=\"/catalog\"").count(), 2);
        let signed_in = render_for(
            template,
            &Visitor {
                authenticated: true,
                csrf: Some("csrf-token".to_owned()),
                auth_dialog: false,
            },
        );
        assert!(!signed_in.contains("href=\"/login\">Войти</a>"));
        assert!(!signed_in.contains("href=\"/register\">Регистрация</a>"));
        assert!(signed_in.contains("href=\"/me/books\">Мои книги</a>"));
        assert!(signed_in.contains("name=\"csrf_token\" value=\"csrf-token\""));
        assert!(signed_in.contains("action=\"/logout\""));
        let signed_in_header = signed_in.split("</header>").next().unwrap();
        for path in [
            "/me/books/new",
            "/me/library",
            "/me",
            "/me/books",
            "/catalog",
        ] {
            assert_eq!(
                signed_in_header
                    .matches(&format!("href=\"{path}\""))
                    .count(),
                2,
                "{path}: exactly one link in each navigation"
            );
        }
        assert!(!signed_in.contains("<!--HEADER_"));
        assert!(!guest.contains("<!--HEADER_"));
    }
}
