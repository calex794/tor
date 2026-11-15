//! Shared taxonomy-label input rules; independent of reports and storage.
use std::{error::Error, fmt};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct InvalidAudienceLabel;

impl fmt::Display for InvalidAudienceLabel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("invalid audience label")
    }
}

impl Error for InvalidAudienceLabel {}

/// Preserve F's optional-filter semantics: blank input becomes no label.
/// Callers selecting one particular group must reject that result separately.
pub fn normalize_audience_label(
    value: Option<String>,
) -> Result<Option<String>, InvalidAudienceLabel> {
    let Some(value) = value else { return Ok(None) };
    // Bound before allocating normalized output, including whitespace-only input.
    if value.len() > 512 || value.chars().any(char::is_control) {
        return Err(InvalidAudienceLabel);
    }
    let clean = value
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase();
    if clean.chars().count() > 40 {
        return Err(InvalidAudienceLabel);
    }
    Ok((!clean.is_empty()).then_some(clean))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn optional_labels_preserve_unicode_and_blank_filter_rules() {
        assert_eq!(normalize_audience_label(None), Ok(None));
        assert_eq!(normalize_audience_label(Some(" ".repeat(512))), Ok(None));
        assert_eq!(
            normalize_audience_label(Some("  МАГИЯ\u{a0}  И Мир  ".into())),
            Ok(Some("магия и мир".into()))
        );
        assert_eq!(
            normalize_audience_label(Some("Я".repeat(40))),
            Ok(Some("я".repeat(40)))
        );
    }

    #[test]
    fn raw_bytes_controls_and_normalized_length_are_bounded() {
        for value in [
            " ".repeat(513),
            "\u{a0}".repeat(257),
            "я".repeat(41),
            "genre\nlabel".into(),
            "genre\tlabel".into(),
            "genre\0label".into(),
            "\u{7f}".into(),
        ] {
            assert_eq!(
                normalize_audience_label(Some(value)),
                Err(InvalidAudienceLabel)
            );
        }
    }
}
