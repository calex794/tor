//! E-04/S013: NEwBOO paired `||spoiler||` policy, without trusted HTML.
//! Source: comment_markup_service.py at NEwBOO 17125ef758.
//! This is presentation only, never an authorisation boundary.

#[derive(Debug, Eq, PartialEq)]
pub enum Segment<'a> {
    Text(&'a str),
    Spoiler(&'a str),
}

/// Empty pairs and unmatched delimiters remain literal, matching NEwBOO.
pub fn parse(body: &str) -> Vec<Segment<'_>> {
    let mut parts = Vec::new();
    let mut rest = body;
    while let Some(start) = rest.find("||") {
        let Some(end) = rest[start + 2..].find("||").map(|n| n + start + 2) else {
            break;
        };
        if start > 0 {
            parts.push(Segment::Text(&rest[..start]));
        }
        parts.push(if end == start + 2 {
            Segment::Text(&rest[start..end + 2])
        } else {
            Segment::Spoiler(&rest[start + 2..end])
        });
        rest = &rest[end + 2..];
    }
    if !rest.is_empty() || parts.is_empty() {
        parts.push(Segment::Text(rest));
    }
    parts
}

fn handle_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_' || b == b'.'
}

/// Same ASCII boundaries and 3..32 handle / 1..19 digit ID grammar as NEwBOO.
/// No identity lookup or notification is implied by masking a mention.
pub fn mask_mentions(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut result = String::new();
    let mut copied = 0;
    for (start, _) in text.match_indices('@') {
        if start < copied
            || (start > 0
                && (bytes[start - 1].is_ascii_alphanumeric()
                    || matches!(bytes[start - 1], b'_' | b'@')))
        {
            continue;
        }
        let tail = &text[start + 1..];
        let length = if let Some(id_tail) = tail.strip_prefix("[user:") {
            let digits = id_tail.bytes().take_while(u8::is_ascii_digit).count();
            if (1..=19).contains(&digits)
                && id_tail.as_bytes()[0] != b'0'
                && id_tail.as_bytes().get(digits) == Some(&b']')
            {
                Some(6 + digits + 1)
            } else {
                None
            }
        } else {
            let n = tail.bytes().take_while(|b| handle_byte(*b)).count();
            (3..=32).contains(&n).then_some(n)
        };
        if let Some(length) = length {
            let end = start + 1 + length;
            if bytes.get(end).is_some_and(|b| handle_byte(*b)) {
                continue;
            }
            result.push_str(&text[copied..start]);
            result.push_str("@пользователь");
            copied = end;
        }
    }
    result.push_str(&text[copied..]);
    result
}

/// Plain text only; future delivery surfaces must HTML-escape at their boundary.
/// Mask BEFORE truncation, so a short preview cannot expose a spoiler prefix.
pub fn delivery_preview(body: &str, max_chars: usize) -> String {
    let mut preview = String::new();
    for part in parse(body) {
        match part {
            Segment::Text(text) => preview.push_str(&mask_mentions(text)),
            Segment::Spoiler(_) => preview.push_str("[спойлер]"),
        }
    }
    let limit = max_chars.max(1);
    if preview.chars().count() <= limit {
        return preview;
    }
    let short: String = preview.chars().take(limit - 1).collect();
    format!("{}…", short.trim_end())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn e_04_newboo_pairs_and_literal_unclosed_markup() {
        assert_eq!(
            parse("до ||секрет <b>|| между ||ещё||"),
            vec![
                Segment::Text("до "),
                Segment::Spoiler("секрет <b>"),
                Segment::Text(" между "),
                Segment::Spoiler("ещё"),
            ]
        );
        assert_eq!(parse("||||"), vec![Segment::Text("||||")]);
        assert_eq!(parse("||незакрытый"), vec![Segment::Text("||незакрытый")]);
        assert_eq!(parse(""), vec![Segment::Text("")]);
    }

    #[test]
    fn e_04_delivery_masks_before_unicode_truncation() {
        assert_eq!(
            delivery_preview("до ||@ivan_k secret|| между ||ещё||", 240),
            "до [спойлер] между [спойлер]"
        );
        assert_eq!(
            delivery_preview("Привет @semi_private", 240),
            "Привет @пользователь"
        );
        assert_eq!(delivery_preview("||SECRET||", 4), "[сп…");
        assert_eq!(delivery_preview("😀😀😀", 2), "😀…");
        assert_eq!(delivery_preview("abc", 0), "…");
        assert_eq!(
            delivery_preview("||незакрытый секрет", 240),
            "||незакрытый секрет"
        );
    }

    #[test]
    fn e_04_mentions_preserve_source_boundaries_and_never_link() {
        assert_eq!(
            mask_mentions("@semi_private @[user:42]"),
            "@пользователь @пользователь"
        );
        assert_eq!(
            mask_mentions("a@abc @@abc @ab @[user:0] @[user:42]x"),
            "a@abc @@abc @ab @[user:0] @[user:42]x"
        );
        assert_eq!(
            mask_mentions(&format!("@{}", "a".repeat(33))),
            format!("@{}", "a".repeat(33))
        );
        assert_eq!(mask_mentions("😀@abc!"), "😀@пользователь!");
        assert_eq!(mask_mentions("<script>"), "<script>");
    }
}
