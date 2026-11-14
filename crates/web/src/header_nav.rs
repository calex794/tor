//! Session-aware navigation for server-rendered pages.
//! The session is checked only on HTML-like GET routes, and never grants access.
use axum::{
    extract::{Request, State},
    http::{header, HeaderMap, HeaderValue, Method, StatusCode},
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

// Read-only administrative reports must never issue a new CSRF cookie on GET.
// Reuse an existing token if present; logout stays available from /me otherwise.
fn may_issue_header_csrf(path: &str) -> bool {
    path != "/admin" && !path.starts_with("/admin/")
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
            let token = crate::auth_cookies::cookie_value(
                request.headers(),
                state.cookies.csrf_name(),
            )
            .filter(|value| state.cookies.valid_csrf(request.headers(), value));
            (token, Vec::new())
        }
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
    response
}

/// Public/demo renders have no verified session context and always show guest links.
pub(crate) fn render(html: String) -> String {
    let visitor = VISITOR.try_with(Clone::clone).unwrap_or_default();
    render_for(&html, &visitor)
}

fn render_for(html: &str, visitor: &Visitor) -> String {
    let (mobile, desktop) = if visitor.authenticated {
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
                r#"<a class="site-mobile-nav__link" href="/me">Кабинет</a>
<a class="site-mobile-nav__link" href="/me/books">Мои книги</a>
{mobile_logout}"#
            ),
            format!(
                r#"<a class="site-nav__link" href="/me/books">Мои книги</a>
<a class="button button--small" href="/me">Кабинет</a>
{desktop_logout}"#
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
    fn admin_reports_remain_cookie_free_without_a_preexisting_csrf_token() {
        assert!(may_issue_header_csrf("/me/books/new"));
        assert!(!may_issue_header_csrf("/admin"));
        assert!(!may_issue_header_csrf("/admin/audience/reading"));
        let html = render_for(
            include_str!("../templates/layout.html"),
            &Visitor {
                authenticated: true,
                csrf: None,
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
