// Auth-return allowlist regression tests.
use super::auth_return_target;

#[test]
fn only_allowlisted_local_paths_can_be_used_after_login() {
    assert_eq!(auth_return_target(None), None);
    assert_eq!(
        auth_return_target(Some("/me/books/new")),
        Some("/me/books/new".to_owned())
    );
    assert_eq!(
        auth_return_target(Some("/read/42")),
        Some("/read/42".to_owned())
    );
    for candidate in [
        "",
        "/",
        "/me",
        "//evil.test",
        "https://evil.test/read/42",
        "javascript:alert(1)",
        "/read/0",
        "/read/042",
        "/read/-1",
        "/read/42/",
        "/read/42?next=//evil.test",
        "/read/42#trick",
        "/read/%34%32",
        "/read/42\\other",
        "/read/42\r\nLocation: https://evil.test",
        "/read/9223372036854775808",
        "/me/books/new/",
        "/me/books/new?next=//evil.test",
        "/me/books/new#fragment",
        "/me/books/new%2F..",
    ] {
        assert_eq!(auth_return_target(Some(candidate)), None, "{candidate}");
    }
}
