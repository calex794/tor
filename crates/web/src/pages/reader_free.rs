//! H-05 / S006: keep published free reading HTML intact, expose only
//! server-verified source offsets to the authenticated reader.
use super::{escape_html, layout, reader_free_comments, reader_free_issues};
use axum::response::Html;
use std::fmt::Write;

struct ProgressView {
    html: String,
    attributes: String,
    subtitle: &'static str,
}

fn progress_view(
    markdown: &str,
    chapter_id: Option<u64>,
    version_id: u64,
    csrf: Option<&str>,
) -> ProgressView {
    // C's immutable reader manifest normalizes CRLF before grouping. Render
    // the same normalized source so each data-md-start is an exact UTF-16 offset.
    let normalized = markdown.replace("\r\n", "\n").replace('\r', "\n");
    let html = tor_reading::domain::renderer::render(&normalized);
    let (Some(chapter_id), Some(csrf)) = (chapter_id, csrf) else {
        return ProgressView {
            html,
            attributes: String::new(),
            subtitle:
                "Читайте без регистрации. Чтобы сохранять место чтения, войдите в личный кабинет.",
        };
    };
    let mut offsets = String::new();
    let mut start = 0_usize;
    for (index, block) in tor_reading::services::progress::reader_blocks(&normalized)
        .iter()
        .enumerate()
    {
        if index > 0 {
            offsets.push(',');
            start += 2; // canonical separator: \n\n, in UTF-16 units
        }
        let length = block.encode_utf16().count();
        write!(offsets, "{start}:{length}").expect("write into String");
        start += length;
    }
    let attributes = format!(
        "data-free-progress-chapter=\"{chapter_id}\" \
         data-free-progress-version=\"{version_id}\" \
         data-free-progress-blocks=\"{offsets}\" \
         data-free-progress-csrf=\"{}\"",
        escape_html(csrf)
    );
    ProgressView {
        html,
        attributes,
        subtitle: "Место чтения сохраняется автоматически. Войти с другого устройства можно с тем же аккаунтом.",
    }
}

/// The public reader can link to the existing authenticated completion route.
/// A link never creates demand, grants access, or assumes that an open was read.
pub fn reader_with_login_link(
    chapter: &tor_reading::services::PublicChapterPage,
    chapter_id: Option<u64>,
) -> Html<String> {
    reader_with_optional_progress(chapter, chapter_id, None)
}

pub fn reader_with_optional_progress(
    chapter: &tor_reading::services::PublicChapterPage,
    chapter_id: Option<u64>,
    csrf: Option<&str>,
) -> Html<String> {
    reader_with_public_features(chapter, chapter_id, csrf, None)
}

/// Reuse the existing trusted text-issues POST; query feedback is display only.
pub fn reader_with_public_features(
    chapter: &tor_reading::services::PublicChapterPage,
    chapter_id: Option<u64>,
    csrf: Option<&str>,
    issue_feedback: Option<&str>,
) -> Html<String> {
    reader_with_public_discussions(chapter, chapter_id, csrf, issue_feedback, None, None)
}

/// HTML includes only E-02 database rows verified against this free edition.
pub fn reader_with_public_discussions(
    chapter: &tor_reading::services::PublicChapterPage,
    chapter_id: Option<u64>,
    csrf: Option<&str>,
    issue_feedback: Option<&str>,
    comments: Option<&tor_discussions::repositories::public_chapter_comments::ChapterCommentPage>,
    comment_feedback: Option<&str>,
) -> Html<String> {
    let previous = chapter
        .previous_chapter_id
        .map(|id| format!("<a rel=\"prev\" href=\"/reader/{id}\">Предыдущая глава</a>"))
        .unwrap_or_default();
    use tor_reading::services::PublicContinuation;
    let (next, continuation_status) = match chapter.continuation() {
        PublicContinuation::ReadNow(id) => (
            format!("<a rel=\"next\" href=\"/reader/{id}\">Следующая глава</a>"),
            "",
        ),
        PublicContinuation::NeedsAccess(id) => (
            format!("<a rel=\"next\" href=\"/reader/{id}\">Следующая глава — платная</a>"),
            "<p class=\"reader-next-state\">Следующая глава опубликована. Для чтения может потребоваться покупка.</p>",
        ),
        PublicContinuation::WaitingForRelease => (
            String::new(),
            "<p class=\"reader-next-state\">Следующих опубликованных глав пока нет.</p>",
        ),
    };
    let return_to_reading = if matches!(
        chapter.continuation(),
        PublicContinuation::WaitingForRelease
    ) {
        chapter_id
            .map(|id| {
                format!(
                    "<p class=\"reader-next-state\"><a class=\"text-link\" href=\"/read/{id}\">Отметить главу прочитанной и попросить продолжение</a></p>"
                )
            })
            .unwrap_or_default()
    } else {
        String::new()
    };
    let continuation_status = format!("{continuation_status}{return_to_reading}");
    let navigation = if previous.is_empty() && next.is_empty() {
        String::new()
    } else {
        format!("<nav class=\"chapter-navigation\" aria-label=\"Переход между главами\">{previous}{next}</nav>")
    };
    let thanks_link = chapter_id
        .map(|id| format!("<p class=\"reader-page__thanks\"><a class=\"text-link\" href=\"/reader/{id}/thanks\">Сказать «Спасибо» за главу</a></p>"))
        .unwrap_or_default();
    let progress = progress_view(&chapter.body_markdown, chapter_id, chapter.version_id, csrf);
    let issues = reader_free_issues::panel(chapter_id, chapter.version_id, csrf, issue_feedback);
    let discussion = reader_free_comments::panel(
        chapter_id,
        chapter.version_id,
        csrf,
        comments,
        comment_feedback,
    );
    let html = include_str!("../../templates/reader.html")
        .replace("<!--BOOK_ID-->", &chapter.book_id.to_string())
        .replace(
            "<!--CHAPTER_ID-->",
            &chapter_id.map_or_else(String::new, |id| id.to_string()),
        )
        .replace("<!--FREE_ISSUES_PANEL-->", &issues)
        .replace("<!--FREE_CHAPTER_COMMENTS-->", &discussion)
        .replace("<!--BOOK_TITLE-->", &escape_html(&chapter.book_title))
        .replace("<!--CHAPTER_TITLE-->", &escape_html(&chapter.chapter_title))
        .replace("<!--CHAPTER_VERSION-->", &chapter.version_no.to_string())
        .replace("<!--FREE_PROGRESS_ATTRS-->", &progress.attributes)
        .replace("<!--READER_SUBTITLE-->", progress.subtitle)
        .replace("<!--CHAPTER_NAVIGATION-->", &navigation)
        .replace("<!--CONTINUATION_STATUS-->", &continuation_status)
        .replace("<!--CHAPTER_THANKS_LINK-->", &thanks_link)
        .replace("<!--CHAPTER_CONTENT-->", &progress.html);
    layout(
        &chapter.chapter_title,
        "Опубликованная бесплатная глава.",
        &html,
    )
}

#[cfg(test)]
mod progress_tests {
    use super::progress_view;

    #[test]
    fn h_05_manifest_offsets_match_unicode_and_canonical_blocks() {
        let source = "т😀a\r\n\r\n**Новая** глава\r\n\r\nКонец";
        let rendered = progress_view(source, Some(101), 607, Some("safe"));
        assert!(rendered.html.contains("data-md-start=\""));
        assert!(rendered.html.contains("т😀a"));
        assert!(rendered
            .attributes
            .contains("data-free-progress-chapter=\"101\""));
        assert!(rendered
            .attributes
            .contains("data-free-progress-version=\"607\""));
        assert!(
            rendered
                .attributes
                .contains("data-free-progress-blocks=\"0:4,6:15,23:5\""),
            "reader_blocks must use the same UTF-16 boundaries as the renderer: {}",
            rendered.attributes
        );
    }

    #[test]
    fn h_05_guest_never_gets_csrf_or_user_position_metadata() {
        let guest = progress_view("Привет", Some(101), 607, None);
        assert!(guest.attributes.is_empty());
        assert!(guest.subtitle.contains("войдите"));
        assert!(guest.html.contains("Привет"));
        let no_id = progress_view("Привет", None, 607, Some("test"));
        assert!(no_id.attributes.is_empty());
        let malicious = progress_view("Глава", Some(101), 607, Some("a\" onload=\"evil"));
        assert!(malicious.attributes.contains("a&quot; onload=&quot;evil"));
        assert!(!malicious.attributes.contains("a\" onload=\"evil"));
    }

    #[test]
    fn h_05_offsets_remain_bounded_for_more_than_200_paragraphs() {
        let source = (0..205)
            .map(|i| format!("абзац {i}"))
            .collect::<Vec<_>>()
            .join("\n\n");
        let view = progress_view(&source, Some(2), 9, Some("token"));
        let raw = view
            .attributes
            .split("data-free-progress-blocks=\"")
            .nth(1)
            .unwrap()
            .split('"')
            .next()
            .unwrap();
        let blocks: Vec<_> = raw.split(',').collect();
        assert!(blocks.len() <= 200 && blocks.len() > 1);
        let mut next = 0;
        for (i, block) in blocks.iter().enumerate() {
            let (start, length) = block.split_once(':').unwrap();
            let start: usize = start.parse().unwrap();
            let length: usize = length.parse().unwrap();
            assert_eq!(start, next + if i == 0 { 0 } else { 2 });
            next = start + length;
        }
        assert_eq!(next, source.encode_utf16().count());
    }
}
