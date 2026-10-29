use tor_search::domain::seo_urls::{BookUrl, TrustedSeoOrigin};

#[test]
fn seo_03_numeric_book_url_is_independent_of_editable_metadata() {
    let book = BookUrl::from_id(42).expect("valid public book identity");
    assert_eq!(book.id(), 42);
    assert_eq!(book.path(), "/books/42");
    assert!(book.matches_path("/books/42"));
    // The caller can change Cyrillic, Chinese or other titles without altering ID.
    for title in ["Название", "变脸武士", "<script>не доверяем</script>"] {
        assert!(!book.path().contains(title));
        assert_eq!(book.path(), "/books/42");
    }
    assert_eq!(
        BookUrl::from_id(u64::MAX).unwrap().path(),
        format!("/books/{}", u64::MAX)
    );
}

#[test]
fn seo_03_only_one_exact_decimal_book_path_is_accepted() {
    assert_eq!(BookUrl::from_id(0), None);
    assert_eq!(BookUrl::from_exact_path("/books/1"), BookUrl::from_id(1));
    for path in [
        "/books/0",
        "/books/01",
        "/books/+1",
        "/books/-1",
        "/books/1/",
        "/books/1?after=5",
        "/books/%31",
        "/books/1%2F",
        "/books/фэнтези",
        "/book/1",
        "/books/18446744073709551616",
        "//books/1",
        "/books/1#toc",
    ] {
        assert_eq!(BookUrl::from_exact_path(path), None, "{path}");
    }
}

#[test]
fn seo_04_trusted_origin_never_comes_from_request_headers() {
    let origin = TrustedSeoOrigin::from_config("https://pandaolate.ru/")
        .expect("configured HTTPS DNS origin");
    let url = origin.book_url(BookUrl::from_id(42).unwrap());
    assert_eq!(url, "https://pandaolate.ru/books/42");
    assert_eq!(
        TrustedSeoOrigin::from_config("https://shadow.pandaolate.ru")
            .unwrap()
            .book_url(BookUrl::from_id(42).unwrap()),
        "https://shadow.pandaolate.ru/books/42"
    );
    for invalid in [
        "",
        "http://pandaolate.ru",
        "https://pandaolate.ru@evil.example",
        "https://pandaolate.ru/path",
        "https://pandaolate.ru//",
        "https://pandaolate.ru?from=Host",
        "https://pandaolate.ru#fragment",
        "https://pandaolate.ru:443",
        "https://evil.example\\@pandaolate.ru",
        "https://invalid..example",
        "https://-bad.example",
    ] {
        assert_eq!(TrustedSeoOrigin::from_config(invalid), None, "{invalid}");
    }
}
