//! Authenticated reader library with explicit new-chapter updates.
use super::{deny, followed_purchase_links, pages, session_token_digest, AuthState, LibraryQuery};
use crate::auth_cookies::private_response;
use axum::{
    extract::{Query, State},
    http::{HeaderMap, StatusCode},
    response::{Redirect, Response},
};

pub(super) async fn my_library(
    State(state): State<AuthState>,
    headers: HeaderMap,
    Query(query): Query<LibraryQuery>,
) -> Response {
    let Some(token) = state.cookies.session_token(&headers) else {
        return private_response(Redirect::to("/login?next=/me/library"), &[]);
    };
    let digest = session_token_digest(&token);
    let user_id = match state.accounts.active_session_user(&digest).await {
        Ok(Some(id)) => id,
        Ok(None) => return private_response(Redirect::to("/login?next=/me/library"), &[]),
        Err(_) => return deny(StatusCode::SERVICE_UNAVAILABLE),
    };
    let books = match state.progress.continue_reading(user_id).await {
        Ok(books) => books,
        Err(_) => return deny(StatusCode::SERVICE_UNAVAILABLE),
    };
    let saved = match state
        .bookmarks
        .list_books(user_id, query.limit.unwrap_or(30), query.after)
        .await
    {
        Ok(saved) => saved,
        Err(tor_reading::services::library::LibraryError::Invalid) => {
            return deny(StatusCode::BAD_REQUEST);
        }
        Err(_) => return deny(StatusCode::SERVICE_UNAVAILABLE),
    };
    // A paid Book need not have a bookmark: purchases are independent facts.
    // Query the same live age, session, moderation and D purchase policies.
    let purchase_books = match state
        .bookmarks
        .purchased_only_books(user_id, &digest, 30, query.purchased_after)
        .await
    {
        Ok(page) => page,
        Err(tor_reading::services::library::LibraryError::Invalid) => {
            return deny(StatusCode::BAD_REQUEST);
        }
        Err(_) => return deny(StatusCode::SERVICE_UNAVAILABLE),
    };
    let cursor = match query.updates_after.as_deref() {
        Some(raw) => match tor_reading::repositories::UpdatesCursor::parse(raw) {
            Some(cursor) => Some(cursor),
            None => return deny(StatusCode::BAD_REQUEST),
        },
        None => None,
    };
    let updates = match state.follow_updates.latest(user_id, cursor).await {
        Ok(updates) => updates,
        Err(_) => return deny(StatusCode::SERVICE_UNAVAILABLE),
    };
    let owned = match followed_purchase_links::for_reader(&state, user_id, &updates).await {
        Ok(owned) => owned,
        Err(_) => return deny(StatusCode::SERVICE_UNAVAILABLE),
    };
    // Resolve only covers attached to currently public, approved BASIC books.
    // Missing or unavailable slots render the illustrated fallback, not a broken image.
    let mut cover_book_ids = books
        .iter()
        .map(|book| book.book_id)
        .chain(saved.items.iter().map(|book| book.book_id))
        .chain(purchase_books.items.iter().map(|book| book.book_id))
        .chain(updates.items.iter().map(|release| release.book_id))
        .filter_map(|id| u64::try_from(id).ok())
        .collect::<Vec<_>>();
    cover_book_ids.sort_unstable();
    cover_book_ids.dedup();
    let mut confirmed_covers = std::collections::HashSet::new();
    if state.cover_preview_storage.is_some() {
        let slots =
            tor_books::repositories::cover_slots::PgCoverSlots::new(state.accounts.pool().clone());
        for batch in cover_book_ids.chunks(100) {
            match slots.public_card_ids(batch).await {
                Ok(ids) => confirmed_covers.extend(ids),
                Err(_) => {
                    // Optional artwork failure must not prevent reading.
                    confirmed_covers.clear();
                    break;
                }
            }
        }
    }
    let (csrf, cookies) = state.cookies.csrf_for_page(&headers);
    // Query-selected panels work with the strict script-src none CSP.
    // Preserve legacy cursors by deriving the panel when view is omitted.
    let view = query.view.as_deref().or_else(|| {
        if query.updates_after.is_some() {
            Some("updates")
        } else if query.purchased_after.is_some() {
            Some("purchased")
        } else if query.after.is_some() {
            Some("saved")
        } else {
            None
        }
    });
    let revisions = if view == Some("updates") {
        let repo = tor_social::repositories::notifications::PgNotifications::new(
            state.accounts.pool().clone(),
        );
        match repo.list_section(user_id, None, 30, "revisions").await {
            Ok(rows) => rows,
            Err(_) => return deny(StatusCode::SERVICE_UNAVAILABLE),
        }
    } else {
        Vec::new()
    };
    let mut page = pages::my_library_with_updates_and_layers_and_purchased_only_selected(
        &books,
        &saved,
        &csrf,
        (state.paid_reader_available, state.layer_pages_enabled),
        state.paid_checkout_enabled,
        (&updates, &owned),
        (&purchase_books, view, Some(&confirmed_covers)),
    );
    // Per-version read receipts are held by E-08 and never overwrite
    // C-16's (user,chapter) first-release acknowledgement.
    page.0 = page.0.replace(
        "<!--RET_REVISION_CARDS-->",
        &pages::library_revision_cards(&revisions, &csrf),
    );
    // Browser preferences are an allowlisted, presentation-only cookie.
    // Keep private account CSP script-src 'none' unchanged.
    apply_private_library_theme(&mut page.0, &headers);
    private_response(page, &cookies)
}

/// Render the preferred *public* color theme without executing scripts on a
/// sensitive authenticated page. Cookie bytes never enter HTML: only a static
/// enum label is substituted after exact matching.
fn apply_private_library_theme(html: &mut String, headers: &HeaderMap) {
    for raw in headers.get_all(axum::http::header::COOKIE).iter() {
        let Ok(raw) = raw.to_str() else {
            continue;
        };
        for item in raw.split(';') {
            let Some((name, raw_mode)) = item.trim().split_once('=') else {
                continue;
            };
            if name != "tor-ui-theme-v1" {
                continue;
            }
            let theme = match raw_mode {
                "system" => "system",
                "light" => "light",
                "dark" => "dark",
                _ => continue,
            };
            *html = html.replacen(
                "<html lang=\"ru\">",
                &format!("<html lang=\"ru\" data-tor-theme=\"{theme}\">"),
                1,
            );
            break;
        }
        if html.contains(" data-tor-theme=") {
            break;
        }
    }
    // The canonical layout contains this asset for public pages, but the
    // private library must not even request it under script-src 'none'.
    *html = html.replacen(
        "<script src=\"/assets/tor-appearance.js\"></script>",
        "",
        1,
    );
}

#[cfg(test)]
mod private_library_appearance_tests {
    use super::apply_private_library_theme;
    use axum::http::{header, HeaderMap, HeaderValue};

    fn rendered(cookie: Option<&'static str>) -> String {
        let mut headers = HeaderMap::new();
        if let Some(cookie) = cookie {
            headers.insert(header::COOKIE, HeaderValue::from_static(cookie));
        }
        let mut html = String::from(
            "<html lang=\"ru\"><head><script src=\"/assets/tor-appearance.js\"></script></head>\
             <body><form action=\"/me/library/1/remove\"></form></body></html>",
        );
        apply_private_library_theme(&mut html, &headers);
        assert!(!html.contains("<script"));
        assert!(html.contains("<form action=\"/me/library/1/remove\">"));
        html
    }

    #[test]
    fn private_library_theme_cookie_is_server_rendered_with_strict_whitelist() {
        for mode in ["light", "dark", "system"] {
            let header = format!("x=unused; tor-ui-theme-v1={mode}; y=other");
            let mut headers = HeaderMap::new();
            headers.insert(header::COOKIE, HeaderValue::from_str(&header).unwrap());
            let mut html = String::from("<html lang=\"ru\"><body>Без скрипта</body></html>");
            apply_private_library_theme(&mut html, &headers);
            assert!(html.starts_with(&format!(
                "<html lang=\"ru\" data-tor-theme=\"{mode}\">"
            )));
        }
        assert_eq!(
            rendered(None).matches("data-tor-theme").count(),
            0,
            "no cookie must retain the OS theme and server-rendered content"
        );
        for hostile in [
            "tor-ui-theme-v1=%22%3E%3Cscript%3E",
            "tor-ui-theme-v1=dark%22",
            "tor-ui-theme-v1=autodark",
            "tor-ui-theme-v1=none",
        ] {
            assert!(!rendered(Some(hostile)).contains("data-tor-theme"));
        }
    }

    #[test]
    fn private_library_does_not_execute_appearance_script() {
        let html = rendered(Some("tor-ui-theme-v1=dark"));
        assert!(html.contains("data-tor-theme=\"dark\""));
        assert!(!html.contains("tor-appearance.js"));
    }
}
