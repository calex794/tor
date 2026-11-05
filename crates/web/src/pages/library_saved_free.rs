//! Private saved-free editions: archive-like read-only view, not public reader.
use super::{escape_html, layout};
use axum::response::Html;
use std::fmt::Write;
use tor_reading::services::library::SavedFreeChapter;

/// Chapter navigation is actor-scoped metadata. Both choices are displayed
/// when a reader owns two distinct immutable editions of the same chapter.
pub fn saved_archive_list(
    book_id: i64,
    page: &tor_reading::services::library::SavedArchivePage,
    paid_reader_available: bool,
) -> Html<String> {
    let mut rows = String::new();
    for item in &page.items {
        let title = escape_html(&item.chapter_title);
        let mut choices = String::new();
        if let Some(version_no) = item.saved_free_version_no {
            write!(
                choices,
                "<a href=\"/me/library/saved/{}\">Сохранённая бесплатная редакция {version_no}</a>",
                item.chapter_id
            )
            .expect("free archive link");
        }
        if let Some(paid) = &item.purchased {
            if !choices.is_empty() {
                choices.push_str(" · ");
            }
            if paid_reader_available {
                write!(
                    choices,
                    "<a href=\"/read/paid/{}?purchase_id={}\">Купленная редакция {}</a>",
                    item.chapter_id, paid.purchase_id, paid.version_no
                )
                .expect("paid archive link");
            } else {
                write!(
                    choices,
                    "<span>Купленная редакция {} — чтение временно недоступно</span>",
                    paid.version_no
                )
                .expect("paid archive unavailable");
            }
        }
        write!(
            rows,
            "<li>Глава {}. {title}<p class=\"library-card__place\">{choices}</p></li>",
            item.chapter_no
        )
        .expect("archive chapter row");
    }
    if rows.is_empty() {
        rows.push_str("<li>Сейчас нет доступных ранее сохранённых или купленных редакций.</li>");
    }
    let next = page
        .next_cursor
        .map(|number| {
            format!(
                "<p><a href=\"/me/library/{book_id}/saved?after={number}\">Следующие главы</a></p>"
            )
        })
        .unwrap_or_default();
    let content = format!(
        "<section class=\"section auth-section\"><div class=\"container\">\
         <h1>Мои сохранённые главы</h1>\
         <p>Здесь перечислены только ваши закреплённые бесплатные редакции и \
         купленные редакции с действующим правом чтения. \
         Новые версии и главы автоматически не открываются.</p>\
         <ul>{rows}</ul>{next}\
         <p><a href=\"/me/library\">Вернуться в библиотеку</a></p>\
         </div></section>"
    );
    layout(
        "Сохранённые главы",
        "Личная библиотека, архив бесплатных и купленных редакций.",
        &content,
    )
}

// The archive cannot execute client scripts (strict CSP). Insert an inert
// fragment at the exact saved UTF-16 offset in the already escaped renderer.
fn escaped_utf16_boundary(encoded: &str, target: usize) -> (usize, usize) {
    let mut byte = 0;
    let mut units = 0;
    while byte < encoded.len() {
        let tail = &encoded[byte..];
        let (step, size) = if tail.starts_with("&amp;") || tail.starts_with("&quot;") {
            (if tail.starts_with("&amp;") { 5 } else { 6 }, 1)
        } else if tail.starts_with("&lt;") || tail.starts_with("&gt;") {
            (4, 1)
        } else if tail.starts_with("&#39;") {
            (5, 1)
        } else {
            let ch = tail.chars().next().expect("valid UTF-8");
            (ch.len_utf8(), ch.len_utf16())
        };
        if units + size > target {
            break;
        }
        byte += step;
        units += size;
    }
    (byte, units)
}

fn saved_source_offset(
    text: &str,
    place: tor_reading::services::progress::ReadingPlace,
) -> Option<usize> {
    use tor_reading::services::progress::{reader_blocks, valid_text_place};
    let blocks = reader_blocks(text);
    valid_text_place(place, &blocks).ok()?;
    let index = usize::try_from(place.block_index).ok()?.checked_sub(1)?;
    let base: usize = blocks
        .iter()
        .take(index)
        .map(|part| part.encode_utf16().count() + 2)
        .sum();
    Some(base + usize::try_from(place.char_offset).ok()?)
}

fn anchor_saved_offset(
    html: &mut String,
    source: &str,
    place: tor_reading::services::progress::ReadingPlace,
) -> bool {
    let Some(target) = saved_source_offset(source, place) else {
        return false;
    };
    let opening = "<span data-md-start=\"";
    let mut search = 0;
    let mut best: Option<(usize, usize)> = None;
    while let Some(relative) = html[search..].find(opening) {
        let n = search + relative + opening.len();
        let Some(end_relative) = html[n..].find("\">") else {
            break;
        };
        let end = n + end_relative;
        let Some(start) = html[n..end].parse::<usize>().ok() else {
            break;
        };
        let content_start = end + 2;
        let Some(close_relative) = html[content_start..].find("</span>") else {
            break;
        };
        let close = content_start + close_relative;
        let encoded = &html[content_start..close];
        let (_, total_units) = escaped_utf16_boundary(encoded, usize::MAX);
        let within = target.saturating_sub(start).min(total_units);
        let (inside_byte, actual_units) = escaped_utf16_boundary(encoded, within);
        let distance = (start + actual_units).abs_diff(target);
        if best.is_none_or(|(previous, _)| distance < previous) {
            best = Some((distance, content_start + inside_byte));
        }
        search = close + "</span>".len();
    }
    if let Some((_, byte)) = best {
        html.insert_str(byte, "<span id=\"saved-resume\"></span>");
        true
    } else {
        false
    }
}

pub fn saved_free_reader(chapter: &SavedFreeChapter, csrf: &str) -> Html<String> {
    let normalized = chapter
        .body_markdown
        .replace("\r\n", "\n")
        .replace('\r', "\n");
    let mut body = tor_reading::domain::renderer::render(&normalized);
    // Keep the canonical source/UTF-16 manifest identical to the public
    // progress reader, but serve a different, pin-gated sync endpoint.
    let mut progress_attributes = String::new();
    let mut progress_notice = String::new();
    let mut script = String::new();
    if chapter.ordinary_deleted {
        let mut offsets = String::new();
        let mut start = 0_usize;
        for (index, block) in tor_reading::services::progress::reader_blocks(&normalized)
            .iter()
            .enumerate()
        {
            if index > 0 {
                offsets.push(',');
                start += 2;
            }
            let length = block.encode_utf16().count();
            write!(offsets, "{start}:{length}").expect("write manifest");
            start += length;
        }
        progress_attributes = format!(
            "data-free-progress-chapter=\"{}\" data-free-progress-version=\"{}\" \
             data-free-progress-blocks=\"{offsets}\" \
             data-free-progress-csrf=\"{}\" data-free-progress-archive=\"1\"",
            chapter.chapter_id,
            chapter.version_id,
            escape_html(csrf)
        );
        progress_notice =
            "<p data-free-progress-notice hidden role=\"status\"></p>".to_owned();
        script = "<script defer src=\"/assets/newboo-archive-progress.js\"></script>".to_owned();
    }

    let resume = if chapter
        .saved_place
        .is_some_and(|place| anchor_saved_offset(&mut body, &normalized, place))
    {
        "<p><a href=\"#saved-resume\">Продолжить с сохранённой позиции</a></p>"
    } else {
        ""
    };
    let title = escape_html(&chapter.chapter_title);
    let book = escape_html(&chapter.book_title);
    let content = format!(
        "<section class=\"section reader-page\" data-free-reader><div class=\"container\">\
         <p class=\"eyebrow\">Сохранённая редакция {}</p>\
         <h1>{book}: глава {}. {title}</h1>\
         <p>Текст закреплён в личной библиотеке. Текущая версия книги может отличаться.</p>{resume}\
         <article class=\"reader-content\" {progress_attributes}>{body}</article>\
         {progress_notice}\
         <p><a href=\"/me/library/{}/saved\">К сохранённым главам</a></p>\
         {script}</div></section>",
        chapter.version_no, chapter.chapter_no, chapter.book_id
    );
    layout(
        &chapter.chapter_title,
        "Ранее сохранённая бесплатная редакция.",
        &content,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mixed_archive_keeps_distinct_free_and_paid_versions_and_escapes_titles() {
        use tor_reading::services::library::{
            SavedArchiveChapter, SavedArchivePage, SavedPurchasedEdition,
        };
        let page = SavedArchivePage {
            items: vec![SavedArchiveChapter {
                chapter_id: 6,
                chapter_no: 3,
                chapter_title: "<script>alert(1)</script> & chapter".into(),
                saved_free_version_no: Some(1),
                purchased: Some(SavedPurchasedEdition {
                    purchase_id: 42,
                    version_no: 2,
                }),
            }],
            next_cursor: Some(3),
        };
        let html = saved_archive_list(9, &page, true).0;
        assert!(html.contains("/me/library/saved/6"));
        assert!(html.contains("/read/paid/6?purchase_id=42"));
        assert!(html.contains("бесплатная редакция 1"));
        assert!(html.contains("Купленная редакция 2"));
        assert!(html.contains("href=\"/me/library/9/saved?after=3\""));
        assert!(html.contains("&lt;script&gt;"));
        assert!(!html.contains("<script>alert(1)</script>"));

        let unavailable = saved_archive_list(9, &page, false).0;
        assert!(unavailable.contains("чтение временно недоступно"));
        assert!(!unavailable.contains("/read/paid/6?purchase_id=42"));
        assert!(unavailable.contains("/me/library/saved/6"));
    }

    #[test]
    fn archived_free_text_uses_sanitized_renderer_and_escapes_titles() {
        let saved = SavedFreeChapter {
            book_id: 1,
            book_title: "<script>alert(1)</script>".into(),
            chapter_id: 6,
            chapter_no: 1,
            chapter_title: "X & Y".into(),
            version_id: 10,
            version_no: 2,
            body_markdown: "**Original V1**".into(),
            ordinary_deleted: false,
            saved_place: None,
        };
        let html = saved_free_reader(&saved, "csrf").0;
        assert!(html.contains("&lt;script&gt;"));
        assert!(!html.contains("<script>alert(1)</script>"));
        assert!(html.contains("X &amp; Y"));
        assert!(html.contains("Original V1"));
        assert!(!html.contains("data-free-progress-archive"));
        assert!(!html.contains("/assets/newboo-archive-progress.js"));
        assert!(html.contains("/me/library/1/saved"));
    }

    #[test]
    fn archive_resume_targets_exact_utf16_offset_in_sanitized_text() {
        use tor_reading::services::progress::ReadingPlace;
        let saved = SavedFreeChapter {
            book_id: 1,
            book_title: "Archive".into(),
            chapter_id: 6,
            chapter_no: 1,
            chapter_title: "Chapter".into(),
            version_id: 10,
            version_no: 1,
            body_markdown: "AB😀&CD".into(),
            ordinary_deleted: true,
            saved_place: Some(ReadingPlace {
                block_index: 1,
                char_offset: 4,
            }),
        };
        let html = saved_free_reader(&saved, "csrf").0;
        assert!(html.contains("href=\"#saved-resume\""));
        assert!(html.contains("data-free-progress-archive=\"1\""));
        assert!(html.contains("/assets/newboo-archive-progress.js"));
        assert!(html.contains("data-free-progress-blocks=\"0:7\""));
        assert!(html.contains("AB😀<span id=\"saved-resume\"></span>&amp;CD"));
        let stale = SavedFreeChapter {
            saved_place: Some(ReadingPlace {
                block_index: 2,
                char_offset: 0,
            }),
            ..saved
        };
        let html = saved_free_reader(&stale, "csrf").0;
        assert!(!html.contains("saved-resume"));
    }
}
