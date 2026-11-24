//! Shared, presentation-only card covers. Callers retain their existing
//! visibility, authorization, cover availability and fallback policies.

/// Keep the decimal identity of both public u64 and repository i64 book IDs.
/// This renderer neither validates identities nor decides image access.
fn card_image(book_id: i128, prevent_drag: bool) -> String {
    let drag = if prevent_drag {
        r#" draggable="false""#
    } else {
        ""
    };
    format!(
        r#"<img src="/covers/{book_id}/card.webp" alt="" loading="lazy" decoding="async"{drag}>"#
    )
}

/// Shared public catalogue and chapter-end mini-card visual, including the
/// existing compact/shelf placeholder. The recommendation caller deliberately
/// retains its current image-request behavior.
pub(crate) fn mini_cover(book_id: impl Into<i128>, has_cover: bool) -> String {
    let content = if has_cover {
        card_image(book_id.into(), true)
    } else {
        "<span>Нет обложки</span>".to_owned()
    };
    format!(r#"<div class="book-mini-card__cover book-shimmer">{content}</div>"#)
}

/// Private library wrapper and eligibility are owned by the library caller.
pub(super) fn library_cover_image(book_id: i64) -> String {
    card_image(book_id.into(), false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mini_cover_preserves_exact_public_markup_and_placeholder() {
        assert_eq!(
            mini_cover(12_u64, true),
            r#"<div class="book-mini-card__cover book-shimmer"><img src="/covers/12/card.webp" alt="" loading="lazy" decoding="async" draggable="false"></div>"#
        );
        assert_eq!(mini_cover(12_i64, true), mini_cover(12_u64, true));
        assert_eq!(
            mini_cover(12_u64, false),
            r#"<div class="book-mini-card__cover book-shimmer"><span>Нет обложки</span></div>"#
        );
        assert!(mini_cover(u64::MAX, true).contains("/covers/18446744073709551615/card.webp"));
    }

    #[test]
    fn library_cover_preserves_existing_drag_and_accessibility_markup() {
        assert_eq!(
            library_cover_image(42),
            r#"<img src="/covers/42/card.webp" alt="" loading="lazy" decoding="async">"#
        );
    }
}
