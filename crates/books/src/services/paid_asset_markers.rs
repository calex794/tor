//! B/S065: exactly the private-image Markdown marker understood by H.
//! Other Markdown and HTML remain literal text, never implicit media access.
//! The B database checks membership before an immutable edition is published.
use std::collections::BTreeSet;
use tor_contracts::paid_delivery::ObjectId;

/// Distinct object UUIDs in valid H-renderable image markers, max 64 per edition.
pub fn referenced_private_image_ids(body: &str) -> Result<Vec<String>, ()> {
    let mut ids = BTreeSet::new();
    for line in body.split('\n') {
        let Some(raw) = line.strip_prefix("![") else {
            continue;
        };
        let Some((caption, target)) = raw.split_once("](asset:") else {
            continue;
        };
        // Match H's [^]\\r\\n]{0,160} bound in UTF-16 code units.
        if caption.encode_utf16().count() > 160 || caption.contains(']') || caption.contains('\r') {
            continue;
        }
        let Some(id) = target.strip_suffix(')') else {
            continue;
        };
        let Ok(uuid) = ObjectId::parse(id) else {
            continue;
        };
        ids.insert(uuid.as_str().to_owned());
        if ids.len() > 64 {
            return Err(());
        }
    }
    Ok(ids.into_iter().collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    const A: &str = "b0b0a500-0001-4000-8000-000000000001";
    const B: &str = "b0b0a500-0002-4000-8000-000000000002";

    #[test]
    fn collects_only_h_renderable_private_images() {
        let body = format!(
            "Текст\n\n![Схема 😀](asset:{A})\n\n![Повтор](asset:{A})\n\n![Другая](asset:{B})\n"
        );
        assert_eq!(
            referenced_private_image_ids(&body),
            Ok(vec![A.to_owned(), B.to_owned()])
        );
    }

    #[test]
    fn rejects_noncanonical_or_non_image_markers_as_plain_text() {
        let body = format!(
            "![external](https://evil.invalid/x.png)\n<img src=x>\n![x](asset:{A})?t=1\n![bad] text (asset:{B})\n"
        );
        assert_eq!(referenced_private_image_ids(&body), Ok(vec![]));
    }

    #[test]
    fn caps_distinct_images_without_counting_repeated_figures() {
        let ids: Vec<String> = (0..65)
            .map(|n| format!("b0b0a500-0001-4000-8000-{n:012x}"))
            .collect();
        let long = ids
            .iter()
            .map(|id| format!("![x](asset:{id})"))
            .collect::<Vec<_>>()
            .join("\n");
        assert!(referenced_private_image_ids(&long).is_err());
        let repeat = format!("![x](asset:{A})\n").repeat(100);
        assert_eq!(
            referenced_private_image_ids(&repeat),
            Ok(vec![A.to_owned()])
        );
    }

    #[test]
    fn rejects_caption_over_h_utf16_limit() {
        let too_long = "😀".repeat(81);
        assert_eq!(
            referenced_private_image_ids(&format!("![{too_long}](asset:{A})")),
            Ok(vec![])
        );
    }
}
