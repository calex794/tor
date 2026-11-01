//! E-08 mail presentation: public B catalog or exact owner-authorized private view.
//! Never copy the source's unscoped include_hidden catalog fallback.
use super::{deny, session_user, AuthState};
use crate::auth_cookies::private_response;
use crate::pages::{escape_html, layout};
use axum::{
    extract::{Path, State},
    http::{HeaderMap, HeaderValue, StatusCode},
    response::{Redirect, Response},
};
use std::collections::{HashMap, HashSet};
use tor_books::{
    repositories::{cover_slots::PgCoverSlots, PgCatalog},
    services::GetPublicBook,
};
use tor_social::domain::Message;

const PER_MESSAGE: usize = 3;
const PER_PAGE: usize = 40;
const CONCURRENT_LOOKUPS: usize = 8;

#[derive(Clone, Debug, serde::Serialize)]
pub(super) struct MailBookPreview {
    pub(super) book_ref: String,
    pub(super) title: String,
    pub(super) url: String,
    pub(super) cover_url: String,
}

#[derive(Clone, Debug, serde::Serialize)]
pub(super) struct PreviewedText {
    pub(super) body_without_book_links: String,
    pub(super) book_previews: Vec<MailBookPreview>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct BookLink {
    start: usize,
    end: usize,
    // Zero means that a legacy alias has not yet passed its actor-bound
    // PostgreSQL resolution. Zero is never a valid canonical Book ID.
    id: u64,
    alias_key: Option<String>,
}

fn boundary(c: u8) -> bool {
    c.is_ascii_whitespace()
        || matches!(
            c,
            b'.' | b','
                | b';'
                | b':'
                | b'!'
                | b'?'
                | b')'
                | b']'
                | b'}'
                | b'>'
                | b'<'
                | b'"'
                | b'\''
        )
}

fn url_end(bytes: &[u8], after_id: usize) -> Option<usize> {
    match bytes.get(after_id).copied() {
        None => Some(after_id),
        Some(b'?' | b'#') => {
            let mut end = after_id + 1;
            while end < bytes.len()
                && !bytes[end].is_ascii_whitespace()
                && !matches!(bytes[end], b'<' | b'>' | b'(' | b')' | b'[' | b']')
            {
                end += 1;
            }
            while end > after_id + 1
                && matches!(
                    bytes[end - 1],
                    b'.' | b',' | b';' | b':' | b'!' | b'?' | b'}' | b'"' | b'\''
                )
            {
                end -= 1;
            }
            Some(end)
        }
        Some(c) if boundary(c) => Some(after_id),
        _ => None,
    }
}

fn extract_links(body: &str, trusted_origin: &str) -> Vec<BookLink> {
    let bytes = body.as_bytes();
    let mut links = Vec::new();
    let mut cursor = 0;
    while let Some(next) = body[cursor..].find("/book") {
        if links.len() >= PER_MESSAGE {
            break;
        }
        let at = cursor + next;
        let path_len = if body[at..].starts_with("/books/") {
            7
        } else if body[at..].starts_with("/book/") {
            6
        } else {
            cursor = at + 5;
            continue;
        };
        // Scan at most one host-length prefix; adversarial messages cannot cause
        // quadratic rescans of all preceding text.
        let mut token_start = at;
        for (index, ch) in body[..at].char_indices().rev().take(256) {
            if ch.is_whitespace() || matches!(ch, '(' | '[' | '<' | '"' | '\'') {
                break;
            }
            token_start = index;
        }
        let prefix = &body[token_start..at];
        // The source accepted any absolute hostname. That let an external
        // URL be silently replaced with a trusted local preview, hiding the
        // original destination from the recipient. Never treat an off-site
        // link as TOR content. The origin is the configured canonical origin,
        // not an untrusted Host or forwarded header.
        let absolute_host = prefix == trusted_origin;
        let Some(start) = (if prefix.is_empty() {
            Some(at)
        } else if absolute_host {
            Some(token_start)
        } else {
            None
        }) else {
            cursor = at + 5;
            continue;
        };
        let first = at + path_len;
        let mut token_end = first;
        while token_end - first <= 128
            && bytes
                .get(token_end)
                .is_some_and(|c| c.is_ascii_alphanumeric() || matches!(c, b'_' | b'-' | b'.'))
        {
            token_end += 1;
        }
        // Sentence-final dots are punctuation, not part of a legacy alias.
        while token_end > first && bytes[token_end - 1] == b'.' {
            token_end -= 1;
        }
        let raw = &body[first..token_end];
        let canonical_id = raw
            .parse::<i64>()
            .ok()
            .filter(|id| *id > 0 && raw == id.to_string())
            .map(|id| id as u64);
        // Legacy mappings are explicit and case-folded ASCII only. Reject
        // ambiguous numeric prefixes (003, 1abc), Unicode, path traversal
        // and overlong attacker-supplied tokens rather than guessing a Book.
        let alias_key = if canonical_id.is_none()
            && raw.len() <= 128
            && raw.as_bytes().first().is_some_and(u8::is_ascii_alphabetic)
            && raw
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'_' | b'-' | b'.'))
        {
            Some(raw.to_ascii_lowercase())
        } else {
            None
        };
        if canonical_id.is_none() && alias_key.is_none() {
            cursor = at + 5;
            continue;
        }
        let Some(end) = url_end(bytes, token_end) else {
            cursor = at + 5;
            continue;
        };
        links.push(BookLink {
            start,
            end,
            id: canonical_id.unwrap_or(0),
            alias_key,
        });
        cursor = end;
    }
    links
}

fn project(
    body: &str,
    links: &[BookLink],
    titles: &HashMap<u64, String>,
    covers: &HashSet<u64>,
    owned_titles: &HashMap<u64, String>,
) -> PreviewedText {
    let mut previews = Vec::new();
    let mut displayed = HashSet::new();
    let mut plain = String::with_capacity(body.len());
    let mut from = 0;
    for link in links {
        let public_title = titles.get(&link.id);
        let Some(title) = public_title.or_else(|| owned_titles.get(&link.id)) else {
            continue;
        };
        if link.start < from {
            continue;
        }
        plain.push_str(&body[from..link.start]);
        from = link.end;
        if displayed.insert(link.id) {
            previews.push(MailBookPreview {
                book_ref: link.id.to_string(),
                title: title.clone(),
                url: if public_title.is_some() {
                    format!("/books/{}", link.id)
                } else {
                    format!("/me/books/{}/preview", link.id)
                },
                cover_url: if public_title.is_some() && covers.contains(&link.id) {
                    format!("/covers/{}/card.webp", link.id)
                } else {
                    String::new()
                },
            });
        }
    }
    if previews.is_empty() {
        return PreviewedText {
            body_without_book_links: body.to_owned(),
            book_previews: previews,
        };
    }
    plain.push_str(&body[from..]);
    PreviewedText {
        body_without_book_links: plain,
        book_previews: previews,
    }
}

/// Private Book mail destination: title only, no owner tools or chapters.
/// Actor membership is checked together with the title to avoid ACL races.
pub(super) async fn page(
    State(state): State<AuthState>,
    Path(id): Path<i64>,
    headers: HeaderMap,
) -> Response {
    let actor = match session_user(&state, &headers).await {
        Ok(Some(actor)) => actor,
        Ok(None) => return private_response(Redirect::to("/login"), &[]),
        Err(status) => return deny(status),
    };
    let catalog = PgCatalog::new(state.accounts.pool().clone());
    let title = match catalog.private_mail_title_for_member(actor, id).await {
        Ok(Some(title)) => title,
        Ok(None) => return deny(StatusCode::NOT_FOUND),
        Err(_) => return deny(StatusCode::SERVICE_UNAVAILABLE),
    };
    let html = layout(
        "Превью книги",
        "Доступно участникам книжного проекта.",
        &format!(
            r#"<section class="me-page"><div class="container"><header class="ui-page-header"><h1>{}</h1></header><p>Просмотр доступен участнику книжного проекта.</p><a href="/mail">Вернуться к переписке</a></div></section>"#,
            escape_html(&title),
        ),
    );
    let mut response = private_response(html, &[]);
    response.headers_mut().insert(
        "content-security-policy",
        HeaderValue::from_static(
            "default-src 'none'; script-src 'none'; style-src 'self'; img-src 'none'; base-uri 'none'; frame-ancestors 'none'",
        ),
    );
    response
}

/// Called only after actor authentication and conversation membership checks.
pub(super) async fn enrich(
    state: &AuthState,
    actor: i64,
    messages: &[Message],
) -> Vec<PreviewedText> {
    let mut links: Vec<_> = messages
        .iter()
        .map(|m| extract_links(&m.body, &state.cookies.origin))
        .collect();
    let mut ids = Vec::new();
    let mut alias_keys = Vec::new();
    let mut seen_ids = HashSet::new();
    let mut seen_aliases = HashSet::new();
    let mut refs = 0;
    for link in links.iter().flatten() {
        if refs == PER_PAGE {
            break;
        }
        if let Some(alias) = &link.alias_key {
            if seen_aliases.insert(alias.clone()) {
                alias_keys.push(alias.clone());
                refs += 1;
            }
        } else if seen_ids.insert(link.id) {
            ids.push(link.id);
            refs += 1;
        }
    }
    let pool = state.accounts.pool().clone();
    // A shared alias is only a pointer, never an access grant. B's repository
    // enforces visibility and active owner ACL *before* returning canonical IDs.
    let alias_ids = PgCatalog::new(pool.clone())
        .resolve_visible_mail_aliases(actor, &alias_keys)
        .await
        .unwrap_or_default();
    for message_links in &mut links {
        for link in message_links {
            if let Some(alias) = &link.alias_key {
                link.id = alias_ids.get(alias).copied().unwrap_or(0);
            }
        }
    }
    for &id in alias_ids.values() {
        if seen_ids.insert(id) {
            ids.push(id);
        }
    }
    let mut titles = HashMap::new();
    let mut missing_public = Vec::new();
    for batch in ids.chunks(CONCURRENT_LOOKUPS) {
        let mut jobs = tokio::task::JoinSet::new();
        for id in batch {
            let id = *id;
            let catalog = PgCatalog::new(pool.clone());
            jobs.spawn(async move { (id, GetPublicBook::new(&catalog).execute(id).await) });
        }
        while let Some(result) = jobs.join_next().await {
            if let Ok((id, outcome)) = result {
                match outcome {
                    Ok(Some(book)) => {
                        titles.insert(id, book.title().to_owned());
                    }
                    Ok(None) => missing_public.push(id),
                    // A failed public lookup must not become an unscoped private lookup.
                    Err(_) => {}
                }
            }
        }
    }
    // Actor-specific B-09 owner, translator or moderator membership is
    // required for private titles and legacy aliases on every read.
    // Revoked members retain the original source link as plain text.
    let mut owned_titles = HashMap::new();
    for batch in missing_public.chunks(CONCURRENT_LOOKUPS) {
        let mut jobs = tokio::task::JoinSet::new();
        for id in batch {
            let id = *id;
            let catalog = PgCatalog::new(pool.clone());
            jobs.spawn(async move {
                (
                    id,
                    catalog
                        .private_mail_title_for_member(actor, id as i64)
                        .await,
                )
            });
        }
        while let Some(result) = jobs.join_next().await {
            if let Ok((id, Ok(Some(title)))) = result {
                owned_titles.insert(id, title);
            }
        }
    }
    let visible_ids: Vec<_> = titles.keys().copied().collect();
    let covers = PgCoverSlots::new(pool)
        .public_card_ids(&visible_ids)
        .await
        .unwrap_or_default();
    messages
        .iter()
        .zip(links.iter())
        .map(|(m, refs)| project(&m.body, refs, &titles, &covers, &owned_titles))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_links(body: &str) -> Vec<BookLink> {
        extract_links(body, "https://tor.test")
    }

    #[test]
    fn e08_only_canonical_numeric_links_and_bounded_public_refs() {
        let body = "See /book/12, https://tor.test/books/34?from=mail. \
                    /books/99/chapters /books/003 https://tor.test/else/books/55 /books/77";
        let refs = test_links(body);
        assert_eq!(
            refs.iter().map(|x| x.id).collect::<Vec<_>>(),
            vec![12, 34, 77]
        );
        assert_eq!(
            &body[refs[1].start..refs[1].end],
            "https://tor.test/books/34?from=mail"
        );
        assert!(test_links("bad/books/3 /books/0 /books/-4 /books/1xyz").is_empty());
    }

    #[test]
    fn e08_repeated_links_and_long_untrusted_prefix_are_bounded() {
        let body = "/books/12 ".repeat(10_000);
        let refs = test_links(&body);
        assert_eq!(refs.len(), PER_MESSAGE);
        assert!(refs.iter().all(|link| link.id == 12));
        let hostile = format!("{}{}", "x".repeat(300_000), "/books/12");
        assert!(test_links(&hostile).is_empty());
    }

    #[test]
    fn e08_preview_removal_preserves_surrounding_whitespace() {
        let body = "  Prefix /books/12 \n";
        let mut titles = HashMap::new();
        titles.insert(12, "Public".to_owned());
        let projected = project(
            body,
            &test_links(body),
            &titles,
            &HashSet::new(),
            &HashMap::new(),
        );
        assert_eq!(projected.body_without_book_links, "  Prefix  \n");
    }

    #[test]
    fn e08_legacy_alias_links_are_bounded_and_never_guess_numeric_keys() {
        let body = "/book/Legacy-Book https://tor.test/books/ALIAS.v2?from=mail. \
                    /books/42 /books/003 /books/1abc /books/../../etc";
        let refs = test_links(body);
        assert_eq!(refs.len(), PER_MESSAGE);
        assert_eq!(refs[0].alias_key.as_deref(), Some("legacy-book"));
        assert_eq!(refs[1].alias_key.as_deref(), Some("alias.v2"));
        assert_eq!(refs[2].id, 42);
        assert!(test_links("/books/003 /books/1abc /books/.hidden").is_empty());
        assert!(test_links(&format!("/books/{}", "a".repeat(129))).is_empty());
    }

    #[test]
    fn e08_legacy_alias_card_only_uses_a_resolved_canonical_book() {
        let mut refs = test_links("/books/legacy-ref and /books/42 and /books/missing");
        assert_eq!(refs.len(), PER_MESSAGE);
        // The only metadata supplied here has already passed actor-bound B
        // lookup; unresolved alias IDs remain zero and their text is retained.
        refs[0].id = 42;
        let titles = HashMap::from([(42, "Published".to_owned())]);
        let projected = project(
            "/books/legacy-ref and /books/42 and /books/missing",
            &refs,
            &titles,
            &HashSet::new(),
            &HashMap::new(),
        );
        assert_eq!(projected.book_previews.len(), 1);
        assert_eq!(projected.book_previews[0].url, "/books/42");
        assert!(!projected
            .body_without_book_links
            .contains("/books/legacy-ref"));
        assert!(projected.body_without_book_links.contains("/books/missing"));
    }
    #[test]
    fn e08_foreign_absolute_links_are_never_rewritten_as_local_books() {
        let body = "external https://evil.test/books/12?ref=mail and \
                    spoof https://tor.test.evil.test/books/12 and \
                    userinfo https://tor.test@evil.test/books/12; \
                    genuine https://tor.test/books/12 and relative /books/13";
        let refs = test_links(body);
        assert_eq!(refs.len(), 2);
        assert_eq!(refs[0].id, 12);
        assert_eq!(refs[1].id, 13);
        let titles = HashMap::from([(12, "Public 12".to_owned()), (13, "Public 13".to_owned())]);
        let output = project(body, &refs, &titles, &HashSet::new(), &HashMap::new());
        assert!(output
            .body_without_book_links
            .contains("https://evil.test/books/12?ref=mail"));
        assert!(output
            .body_without_book_links
            .contains("https://tor.test.evil.test/books/12"));
        assert!(output
            .body_without_book_links
            .contains("https://tor.test@evil.test/books/12"));
        assert_eq!(output.book_previews.len(), 2);
        assert_eq!(output.book_previews[0].url, "/books/12");
        assert_eq!(output.book_previews[1].url, "/books/13");
        assert!(!output
            .body_without_book_links
            .contains("genuine https://tor.test/books/12"));
    }

    #[test]
    fn e08_owned_draft_preview_never_gets_a_public_cover() {
        let body = "/books/13";
        let owned = HashMap::from([(13, "Owner-only draft".to_owned())]);
        let covers = HashSet::from([13]);
        let projected = project(body, &test_links(body), &HashMap::new(), &covers, &owned);
        assert_eq!(projected.body_without_book_links, "");
        assert_eq!(projected.book_previews.len(), 1);
        assert_eq!(projected.book_previews[0].title, "Owner-only draft");
        assert_eq!(projected.book_previews[0].url, "/me/books/13/preview");
        assert_eq!(projected.book_previews[0].cover_url, "");
        let denied = project(
            body,
            &test_links(body),
            &HashMap::new(),
            &covers,
            &HashMap::new(),
        );
        assert!(denied.book_previews.is_empty());
        assert_eq!(denied.body_without_book_links, body);
    }

    #[test]
    fn e08_hidden_ref_stays_text_and_only_public_ref_gets_a_cover() {
        let body = "Public /books/12 and hidden /books/13 plus /book/12";
        let mut titles = HashMap::new();
        titles.insert(12, "<img onerror=alert(1)>".into());
        let covers = HashSet::from([12]);
        let projected = project(body, &test_links(body), &titles, &covers, &HashMap::new());
        assert_eq!(projected.book_previews.len(), 1);
        assert_eq!(projected.book_previews[0].url, "/books/12");
        assert_eq!(projected.book_previews[0].cover_url, "/covers/12/card.webp");
        assert!(projected.body_without_book_links.contains("/books/13"));
        assert!(!projected.body_without_book_links.contains("/books/12"));
        assert!(!projected.body_without_book_links.contains("/book/12"));
        assert!(!projected.body_without_book_links.contains("onerror"));
    }
}
