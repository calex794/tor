//! Restricted, server-side Markdown for reader text. No raw HTML is trusted.
#![forbid(unsafe_code)]
use std::fmt::Write;

fn escape_html(value: &str) -> String {
    let mut escaped = String::with_capacity(value.len());
    for ch in value.chars() {
        match ch {
            '&' => escaped.push_str("&amp;"),
            '<' => escaped.push_str("&lt;"),
            '>' => escaped.push_str("&gt;"),
            '"' => escaped.push_str("&quot;"),
            '\'' => escaped.push_str("&#39;"),
            _ => escaped.push(ch),
        }
    }
    escaped
}

/// At most 50,000 Unicode scalar values per public reader response.
pub const MAX_READER_CHARS: usize = 50_000;

/// A boundary is checked before HTML expansion. Legacy data is never truncated.
pub fn valid_reader_length(source: &str) -> bool {
    source.chars().take(MAX_READER_CHARS + 1).count() <= MAX_READER_CHARS
}

/// One linear pass over UTF-8 replaces repeated O(n^2) prefix decoding.
/// Only UTF-8 character boundaries are used as positions.
fn utf16_offsets(source: &str) -> Vec<usize> {
    let mut result = vec![0; source.len() + 1];
    let mut count = 0;
    for (byte_index, ch) in source.char_indices() {
        result[byte_index] = count;
        count += ch.len_utf16();
    }
    result[source.len()] = count;
    result
}

/// Every visible source segment retains its original UTF-16 offset inside a
/// reading block. The reader can restore older bookmarks after formatting.
fn text_segment(offsets: &[usize], out: &mut String, _block: &str, byte_start: usize, text: &str) {
    if text.is_empty() {
        return;
    }
    let start = offsets[byte_start];
    write!(
        out,
        "<span data-md-start=\"{start}\">{}</span>",
        escape_html(text)
    )
    .expect("writing into String");
}

fn safe_link(url: &str) -> bool {
    if url.is_empty()
        || url.len() > 2048
        || url.chars().any(|c| {
            c.is_control()
                || c.is_whitespace()
                || matches!(c, '<' | '>' | '"' | '\'' | '\\' | '{' | '}' | '\u{60}')
        })
    {
        return false;
    }
    if let Some(host_path) = url
        .strip_prefix("https://")
        .or_else(|| url.strip_prefix("http://"))
    {
        let host = host_path.split(['/', '?', '#']).next().unwrap_or("");
        return !host.is_empty() && !host.starts_with('.') && !host.contains('@');
    }
    (url.starts_with('/') && !url.starts_with("//")) || url.starts_with('#')
}

/// Avoid quadratic searches in malformed user Markdown, even for 50K text.
fn short_prefix(text: &str) -> &str {
    let mut len = text.len().min(2048);
    while !text.is_char_boundary(len) {
        len -= 1;
    }
    &text[..len]
}

/// Limited depth and no raw HTML keep the parser bounded and non-executable.
fn inline(offsets: &[usize], out: &mut String, block: &str, line: &str, base: usize, depth: u8) {
    let mut i = 0;
    let mut plain = 0;
    while i < line.len() {
        let rest = &line[i..];
        let candidate = short_prefix(rest);
        if depth < 6 {
            // Links are explicit and protocol restricted. Images are not parsed.
            if rest.starts_with('[') && (i == 0 || !line[..i].ends_with('!')) {
                if let Some(label_end) = candidate.find("](") {
                    if let Some(end) = candidate[label_end + 2..].find(')') {
                        let label = &rest[1..label_end];
                        let href = &rest[label_end + 2..label_end + 2 + end];
                        if !label.is_empty() && safe_link(href) {
                            text_segment(offsets, out, block, base + plain, &line[plain..i]);
                            write!(
                                out,
                                "<a href=\"{}\" rel=\"nofollow noopener noreferrer\">",
                                escape_html(href)
                            )
                            .expect("writing into String");
                            inline(offsets, out, block, label, base + i + 1, depth + 1);
                            out.push_str("</a>");
                            i += label_end + 3 + end;
                            plain = i;
                            continue;
                        }
                    }
                }
            }
            // Inline code is literal; nested markup is never interpreted here.
            if rest.starts_with('\u{60}') {
                if let Some(end) = candidate[1..].find('\u{60}') {
                    text_segment(offsets, out, block, base + plain, &line[plain..i]);
                    out.push_str("<code>");
                    text_segment(offsets, out, block, base + i + 1, &rest[1..1 + end]);
                    out.push_str("</code>");
                    i += end + 2;
                    plain = i;
                    continue;
                }
            }
            let mut consumed = false;
            for (marker, tag) in [("**", "strong"), ("__", "strong"), ("*", "em"), ("_", "em")] {
                // Markdown underscores inside ordinary words are not formatting.
                if marker.starts_with('_')
                    && line[..i].chars().last().is_some_and(char::is_alphanumeric)
                {
                    continue;
                }
                if rest.starts_with(marker)
                    && !(marker.len() == 1 && rest.starts_with(&marker.repeat(2)))
                {
                    if let Some(end) = candidate[marker.len()..].find(marker) {
                        if end > 0 {
                            text_segment(offsets, out, block, base + plain, &line[plain..i]);
                            write!(out, "<{tag}>").expect("writing into String");
                            inline(
                                offsets,
                                out,
                                block,
                                &rest[marker.len()..marker.len() + end],
                                base + i + marker.len(),
                                depth + 1,
                            );
                            write!(out, "</{tag}>").expect("writing into String");
                            i += marker.len() * 2 + end;
                            plain = i;
                            consumed = true;
                            break;
                        }
                    }
                }
            }
            if consumed {
                continue;
            }
        }
        i += rest.chars().next().expect("inside UTF-8 string").len_utf8();
    }
    text_segment(offsets, out, block, base + plain, &line[plain..]);
}

fn list_prefix(line: &str) -> Option<(&'static str, usize)> {
    if line.starts_with("- ") || line.starts_with("* ") || line.starts_with("+ ") {
        return Some(("ul", 2));
    }
    let digits = line.bytes().take_while(u8::is_ascii_digit).count();
    if (1..=9).contains(&digits) && line[digits..].starts_with(". ") {
        return Some(("ol", digits + 2));
    }
    None
}

fn heading_prefix(line: &str) -> Option<(usize, usize)> {
    let n = line.bytes().take_while(|b| *b == b'#').count();
    if (1..=6).contains(&n) && line[n..].starts_with(' ') {
        Some((n, n + 1))
    } else {
        None
    }
}

fn special(line: &str) -> bool {
    heading_prefix(line).is_some()
        || list_prefix(line).is_some()
        || line.starts_with("> ")
        || line.starts_with("\u{60}\u{60}\u{60}")
}

/// Supports a small, documented subset: headings, paragraphs, emphasis,
/// inline code, lists, quotations, fenced code and safe links.
/// Everything else (including raw HTML) is displayed as escaped text.
/// Render a published chapter using a fixed set of non-executable HTML tags.
/// Raw HTML, images, embedded CSS, scripts and unsafe URL schemes stay escaped.
pub fn render(source: &str) -> String {
    let offset_map = utf16_offsets(source);
    let offsets = offset_map.as_slice();
    let mut lines = Vec::new();
    let mut offset = 0;
    for part in source.split_inclusive('\n') {
        lines.push((offset, part.strip_suffix('\n').unwrap_or(part)));
        offset += part.len();
    }
    let mut out = String::with_capacity(source.len() + 256);
    let mut i = 0;
    while i < lines.len() {
        let (base, line) = lines[i];
        if line.trim().is_empty() {
            i += 1;
            continue;
        }
        if line.starts_with("\u{60}\u{60}\u{60}") {
            out.push_str("<pre><code>");
            i += 1;
            while i < lines.len() && !lines[i].1.starts_with("\u{60}\u{60}\u{60}") {
                let (line_start, raw) = lines[i];
                text_segment(offsets, &mut out, source, line_start, raw);
                if i + 1 < lines.len() {
                    out.push('\n');
                }
                i += 1;
            }
            out.push_str("</code></pre>");
            if i < lines.len() {
                i += 1;
            }
            continue;
        }
        if let Some((level, prefix)) = heading_prefix(line) {
            write!(out, "<h{level}>").expect("writing into String");
            inline(offsets, &mut out, source, &line[prefix..], base + prefix, 0);
            write!(out, "</h{level}>").expect("writing into String");
            i += 1;
            continue;
        }
        if let Some((tag, _)) = list_prefix(line) {
            write!(out, "<{tag}>").expect("writing into String");
            while i < lines.len() {
                let (pos, current) = lines[i];
                let Some((kind, prefix)) = list_prefix(current) else {
                    break;
                };
                if kind != tag {
                    break;
                }
                out.push_str("<li>");
                inline(
                    offsets,
                    &mut out,
                    source,
                    &current[prefix..],
                    pos + prefix,
                    0,
                );
                out.push_str("</li>");
                i += 1;
            }
            write!(out, "</{tag}>").expect("writing into String");
            continue;
        }
        if line.starts_with("> ") {
            out.push_str("<blockquote><p>");
            while i < lines.len() && lines[i].1.starts_with("> ") {
                if i > 0 && lines[i - 1].1.starts_with("> ") {
                    out.push(' ');
                }
                inline(
                    offsets,
                    &mut out,
                    source,
                    &lines[i].1[2..],
                    lines[i].0 + 2,
                    0,
                );
                i += 1;
            }
            out.push_str("</p></blockquote>");
            continue;
        }
        out.push_str("<p>");
        inline(offsets, &mut out, source, line, base, 0);
        i += 1;
        while i < lines.len() && !lines[i].1.trim().is_empty() && !special(lines[i].1) {
            out.push(' ');
            inline(offsets, &mut out, source, lines[i].1, lines[i].0, 0);
            i += 1;
        }
        out.push_str("</p>");
    }
    out
}

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
