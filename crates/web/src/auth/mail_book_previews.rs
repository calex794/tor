//! E-08 mail presentation. Only public books authorized by B become previews.
//! Do not copy the source's unscoped include_hidden catalog fallback.
use super::AuthState;
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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct BookLink {
    start: usize,
    end: usize,
    id: u64,
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

fn extract_links(body: &str) -> Vec<BookLink> {
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
        let absolute_host = prefix
            .strip_prefix("https://")
            .or_else(|| prefix.strip_prefix("http://"))
            .is_some_and(|host| {
                !host.is_empty()
                    && !host
                        .chars()
                        .any(|c| matches!(c, '/' | '?' | '#' | '@') || c.is_whitespace())
            });
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
        let mut end_id = first;
        while bytes.get(end_id).is_some_and(u8::is_ascii_digit) {
            end_id += 1;
        }
        let raw = &body[first..end_id];
        let id = raw
            .parse::<i64>()
            .ok()
            .filter(|id| *id > 0 && raw == id.to_string())
            .map(|id| id as u64);
        let Some(id) = id else {
            cursor = at + 5;
            continue;
        };
        let Some(end) = url_end(bytes, end_id) else {
            cursor = at + 5;
            continue;
        };
        links.push(BookLink { start, end, id });
        cursor = end;
    }
    links
}

fn project(
    body: &str,
    links: &[BookLink],
    titles: &HashMap<u64, String>,
    covers: &HashSet<u64>,
) -> PreviewedText {
    let mut previews = Vec::new();
    let mut displayed = HashSet::new();
    let mut plain = String::with_capacity(body.len());
    let mut from = 0;
    for link in links {
        let Some(title) = titles.get(&link.id) else {
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
                url: format!("/books/{}", link.id),
                cover_url: if covers.contains(&link.id) {
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

/// Called only after actor authentication and conversation membership checks.
pub(super) async fn enrich(state: &AuthState, messages: &[Message]) -> Vec<PreviewedText> {
    let links: Vec<_> = messages.iter().map(|m| extract_links(&m.body)).collect();
    let mut ids = Vec::new();
    let mut seen = HashSet::new();
    for link in links.iter().flatten() {
        if seen.insert(link.id) {
            if ids.len() == PER_PAGE {
                break;
            }
            ids.push(link.id);
        }
    }
    let pool = state.accounts.pool().clone();
    let mut titles = HashMap::new();
    for batch in ids.chunks(CONCURRENT_LOOKUPS) {
        let mut jobs = tokio::task::JoinSet::new();
        for id in batch {
            let id = *id;
            let catalog = PgCatalog::new(pool.clone());
            jobs.spawn(async move { (id, GetPublicBook::new(&catalog).execute(id).await) });
        }
        while let Some(result) = jobs.join_next().await {
            if let Ok((id, Ok(Some(book)))) = result {
                titles.insert(id, book.title().to_owned());
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
        .map(|(m, refs)| project(&m.body, refs, &titles, &covers))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn e08_only_canonical_numeric_links_and_bounded_public_refs() {
        let body = "See /book/12, https://tor.test/books/34?from=mail. \
                    /books/99/chapters /books/003 https://tor.test/else/books/55 /books/77";
        let refs = extract_links(body);
        assert_eq!(
            refs.iter().map(|x| x.id).collect::<Vec<_>>(),
            vec![12, 34, 77]
        );
        assert_eq!(
            &body[refs[1].start..refs[1].end],
            "https://tor.test/books/34?from=mail"
        );
        assert!(extract_links("bad/books/3 /books/0 /books/-4 /books/1xyz").is_empty());
    }

    #[test]
    fn e08_repeated_links_and_long_untrusted_prefix_are_bounded() {
        let body = "/books/12 ".repeat(10_000);
        let refs = extract_links(&body);
        assert_eq!(refs.len(), PER_MESSAGE);
        assert!(refs.iter().all(|link| link.id == 12));
        let hostile = format!("{}{}", "x".repeat(300_000), "/books/12");
        assert!(extract_links(&hostile).is_empty());
    }

    #[test]
    fn e08_preview_removal_preserves_surrounding_whitespace() {
        let body = "  Prefix /books/12 \n";
        let mut titles = HashMap::new();
        titles.insert(12, "Public".to_owned());
        let projected = project(body, &extract_links(body), &titles, &HashSet::new());
        assert_eq!(projected.body_without_book_links, "  Prefix  \n");
    }

    #[test]
    fn e08_hidden_ref_stays_text_and_only_public_ref_gets_a_cover() {
        let body = "Public /books/12 and hidden /books/13 plus /book/12";
        let mut titles = HashMap::new();
        titles.insert(12, "<img onerror=alert(1)>".into());
        let covers = HashSet::from([12]);
        let projected = project(body, &extract_links(body), &titles, &covers);
        assert_eq!(projected.book_previews.len(), 1);
        assert_eq!(projected.book_previews[0].url, "/books/12");
        assert_eq!(projected.book_previews[0].cover_url, "/covers/12/card.webp");
        assert!(projected.body_without_book_links.contains("/books/13"));
        assert!(!projected.body_without_book_links.contains("/books/12"));
        assert!(!projected.body_without_book_links.contains("/book/12"));
        assert!(!projected.body_without_book_links.contains("onerror"));
    }
}
