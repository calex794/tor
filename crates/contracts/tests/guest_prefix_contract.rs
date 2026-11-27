use tor_contracts::{
    guest_prefix::{GuestPrefix, GuestPrefixError, MAX_PREFIX_BYTES, MAX_PREFIX_SCALARS},
    reader_text::{render, MAX_READER_CHARS},
};

#[test]
fn auth_read_prefix_uses_reader_tokens_not_link_destinations() {
    let source = format!(
        "# Глава\n\n**Привет** [текст](https://example.test/PRIVATE_TARGET)\n\n{} HIDDEN_TAIL",
        "слово ".repeat(100)
    );
    let projection = GuestPrefix::from_source(&source).unwrap();
    let visible = format!("Глава\n\nПривет текст\n\n{}HIDDEN_TAIL", "слово ".repeat(100));
    assert_eq!(projection.visible_scalar_count(), visible.chars().count());
    for percent in [10, 20, 30, 50] {
        let excerpt = projection.excerpt(percent).unwrap();
        assert!(visible.starts_with(excerpt));
        assert!(excerpt.starts_with("Глава\n\nПривет текст"));
        assert!(!excerpt.contains("PRIVATE_TARGET"));
        assert!(!excerpt.contains("HIDDEN_TAIL"));
        assert!(!excerpt.contains("**"));
        assert!(excerpt.chars().count() <= visible.chars().count() * usize::from(percent) / 100);
    }
    // Historical renderer still emits its original link and source offsets.
    assert_eq!(
        render("# A\n\n**B** &"),
        "<h1><span data-md-start=\"2\">A</span></h1>\
         <p><strong><span data-md-start=\"7\">B</span></strong>\
         <span data-md-start=\"10\"> &amp;</span></p>"
    );
}

#[test]
fn auth_read_tiny_and_unicode_clusters_never_leak_full_text() {
    for source in ["", " ", "a", "a\u{0301}b", "🇷🇺x"] {
        let projection = GuestPrefix::from_source(source).unwrap();
        assert_eq!(projection.excerpt(50).unwrap(), "");
    }
    let family = GuestPrefix::from_source("😀👩‍👩‍👧‍👦结束").unwrap();
    assert_eq!(family.excerpt(50).unwrap(), "😀");
    let pair = GuestPrefix::from_source("ab").unwrap();
    assert_eq!(pair.excerpt(50).unwrap(), "a");
    assert_eq!(pair.excerpt(20).unwrap(), "");
    let rtl = GuestPrefix::from_source("שלום עולם שלום עולם").unwrap();
    assert!("שלום עולם שלום עולם".starts_with(rtl.excerpt(50).unwrap()));
    assert_eq!(pair.excerpt(100), Err(GuestPrefixError::InvalidPercent));
}

#[test]
fn auth_read_prefix_caps_and_determinism() {
    for char in ['章', '😀'] {
        let source = char.to_string().repeat(MAX_READER_CHARS);
        let first = GuestPrefix::from_source(&source).unwrap();
        let again = GuestPrefix::from_source(&source).unwrap();
        assert_eq!(first.visible_scalar_count(), MAX_READER_CHARS);
        assert_eq!(first.maximum_prefix(), again.maximum_prefix());
        assert_eq!(first.maximum_prefix().chars().count(), MAX_PREFIX_SCALARS);
        assert!(first.maximum_prefix().len() <= MAX_PREFIX_BYTES);
        for percent in [10, 20, 30, 50] {
            assert_eq!(first.excerpt(percent), again.excerpt(percent));
            assert!(first.excerpt(percent).unwrap().len() <= MAX_PREFIX_BYTES);
        }
    }
    assert!(matches!(
        GuestPrefix::from_source(&"x".repeat(MAX_READER_CHARS + 1)),
        Err(GuestPrefixError::SourceTooLong)
    ));
}

#[test]
fn auth_read_unsupported_literal_markup_returns_no_projection() {
    for source in [
        "Начало ![изображение](https://storage.example/private-key) конец",
        "Начало [опасная](javascript:alert(1)) конец",
        "Начало <img src='private-key'> конец",
        "Начало [незакрытая](private-key",
        "Начало <script>secret</script> конец",
    ] {
        assert!(matches!(
            GuestPrefix::from_source(source),
            Err(GuestPrefixError::UnsupportedLiteralMarkup)
        ));
    }
    let math = GuestPrefix::from_source("1 < 2, this is visible prose.").unwrap();
    assert!(math.visible_scalar_count() > 0);
}
