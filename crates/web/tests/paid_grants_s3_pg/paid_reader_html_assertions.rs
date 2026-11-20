//! S034: purchased reader HTML contract, moved out of the main S3 integration test.
use super::{CHAPTER_TITLE, MARKDOWN};

pub(super) fn verify(
    reader_html: &str,
    chapter_two: impl std::fmt::Display,
    purchase_two_id: impl std::fmt::Display,
) {
    assert!(reader_html.contains("data-paid-chapter-reader"));
    assert!(reader_html.contains("data-paid-toc"));
    assert!(reader_html.contains("Оглавление книги"));
    // Republished Book restores public reader metadata.
    assert!(reader_html.contains("data-toc-state=\"free\""));
    assert!(reader_html.contains("data-toc-state=\"locked\""));
    assert!(reader_html.contains(&format!(
        "/read/paid/{chapter_two}?purchase_id={purchase_two_id}"
    )));
    assert!(reader_html.contains("Вторая купленная глава &lt;script&gt;опасно&lt;/script&gt;"));
    assert!(!reader_html.contains("<script>опасно</script>"));
    assert!(reader_html.contains("<body class=\"reader-standalone\">"));
    assert!(reader_html.contains("<main id=\"main\" class=\"reader-standalone__main\">"));
    assert!(!reader_html.contains("class=\"site-header\""));
    assert!(!reader_html.contains("class=\"site-footer\""));
    assert!(reader_html.contains("Невбу &lt;img src=x onerror=window.__xss=true&gt; книга"));
    assert!(reader_html.contains(CHAPTER_TITLE));
    assert!(reader_html.contains("Глава 1. Редакция «V1»"));
    assert!(reader_html.contains("Купленная редакция 1"));
    assert!(!reader_html.contains("<img src=x"));
    assert!(!reader_html.contains(MARKDOWN));
    assert!(!reader_html.contains("dek_b64u"));
}
