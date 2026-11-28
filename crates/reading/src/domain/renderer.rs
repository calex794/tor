//! Reading facade for the shared, byte-compatible restricted Markdown renderer.
#![forbid(unsafe_code)]
pub use tor_contracts::reader_text::{render, valid_reader_length, MAX_READER_CHARS};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn basics_and_unicode_offsets() {
        let result = render("# Глава\n\n**Привет** *мир* и \u{60}код\u{60}");
        assert!(result.contains("<h1><span data-md-start=\"2\">Глава</span></h1>"));
        assert!(result.contains("<strong><span data-md-start=\""));
        assert!(result.contains("<em>"));
        assert!(result.contains("<code>"));
        assert!(render("- первый\n- второй\n\n1. один\n2. два").contains("<ol><li>"));
        assert!(render("> цитата").contains("<blockquote>"));
        assert!(render("NEW_PUBLISHED_TEXT").contains("NEW_PUBLISHED_TEXT"));
        assert!(render("_курсив_").contains("<em>"));
    }

    #[test]
    fn refuses_script_injection_and_restricts_links() {
        let result = render(
            "<script>alert(1)</script>\n\n[опасная](javascript:alert(1)) \
             [опасная](data:text/html,hi) [нормальная](https://example.org/?a=1&b=2)\
             \n\n<img src=x onerror=alert(1)>",
        );
        assert!(!result.contains("<script>"));
        assert!(!result.contains("<img "));
        assert!(!result.contains("href=\"javascript:"));
        assert!(!result.contains("href=\"data:"));
        assert!(result.contains("href=\"https://example.org/?a=1&amp;b=2\""));
        assert!(result.contains("&lt;img"));
        assert!(!render("[x](//evil.test)").contains("<a "));
        assert!(!render("[x](HTTPS://evil.test)").contains("<a "));
        assert!(!render("[x](https://evil.test\\\"onclick)").contains("<a "));
    }

    #[test]
    fn full_input_is_bounded_and_never_embeds_unsafe_html() {
        let huge = "x".repeat(50_000);
        let html = render(&huge);
        assert!(html.len() < 51_000);
        assert!(html.starts_with("<p>"));
        assert!(
            render("\u{60}\u{60}\u{60}html\n<script>x</script>\n\u{60}\u{60}\u{60}")
                .contains("&lt;script&gt;")
        );
    }

    #[test]
    fn unicode_positions_remain_exact_with_many_short_lines() {
        let source = (0..10_000).map(|_| "😀").collect::<Vec<_>>().join("\n");
        assert!(valid_reader_length(&source));
        let last = source.rfind('😀').expect("last emoji");
        let expected = source[..last].encode_utf16().count();
        let html = render(&source);
        assert!(html.contains(&format!("data-md-start=\"{expected}\">😀")));
        assert!(!html.contains("<script"));
    }

    #[test]
    fn source_limit_is_in_unicode_characters_not_bytes() {
        assert!(valid_reader_length(&"😀".repeat(MAX_READER_CHARS)));
        assert!(!valid_reader_length(&"😀".repeat(MAX_READER_CHARS + 1)));
        for attack in [
            "<svg onload=alert(1)>",
            "<iframe srcdoc=\"x\"></iframe>",
            "[link](java\u{0009}script:alert(1))",
            "[link](https://example.org/\" onclick=\"evil)",
        ] {
            let html = render(attack);
            assert!(!html.contains("<svg"));
            assert!(!html.contains("<iframe"));
            // Escaped source may contain " onclick=" as harmless visible text.
            // It must not become an executable attribute on a generated link.
            assert!(!html.contains("<a href=\"https://example.org/"));
            assert!(!html.contains("href=\"javascript:"));
        }
    }
}
