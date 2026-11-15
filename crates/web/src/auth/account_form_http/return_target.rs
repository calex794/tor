//! Explicit post-auth destinations, never an arbitrary local or external URL.

pub(super) fn auth_return_target(raw: Option<&str>) -> Option<String> {
    let raw = raw?;
    if matches!(raw, "/me/library" | "/me/books/new") {
        return Some(raw.to_owned());
    }
    let prefix = ["/read/", "/reader/"]
        .into_iter()
        .find(|prefix| raw.starts_with(*prefix))?;
    let raw_id = raw.strip_prefix(prefix)?;
    if raw_id.is_empty()
        || raw_id.starts_with('0')
        || !raw_id.bytes().all(|byte| byte.is_ascii_digit())
    {
        return None;
    }
    let id = raw_id.parse::<i64>().ok().filter(|value| *value > 0)?;
    Some(format!("{prefix}{id}"))
}

#[cfg(test)]
mod tests {
    use super::auth_return_target;

    #[test]
    fn allow_only_fixed_account_and_canonical_reader_paths() {
        assert_eq!(auth_return_target(None), None);
        for target in ["/me/library", "/me/books/new", "/read/42", "/reader/42"] {
            assert_eq!(auth_return_target(Some(target)).as_deref(), Some(target));
        }
    }

    #[test]
    fn reject_external_urls_and_ambiguous_library_paths() {
        for target in [
            "",
            "/",
            "/me",
            "//evil.test",
            "https://evil.test/reader/42",
            "javascript:alert(1)",
            "/me/books/new/",
            "/me/books/new?next=//evil.test",
            "/me/books/new#fragment",
            "/me/books/new%2F..",
            "/me/library/",
            "/me/library?next=//evil.test",
            "/me/library#trick",
            "/me/%6cibrary",
            "/me/library\r\nLocation: https://evil.test",
        ] {
            assert_eq!(auth_return_target(Some(target)), None, "{target:?}");
        }
    }

    #[test]
    fn reject_malformed_ids_queries_fragments_and_header_injection() {
        for prefix in ["/read/", "/reader/"] {
            for suffix in [
                "",
                "0",
                "042",
                "-1",
                "42/",
                "42?next=//evil.test",
                "42#trick",
                "%34%32",
                "42\\other",
                "42\r\nLocation: https://evil.test",
                "9223372036854775808",
            ] {
                let target = format!("{prefix}{suffix}");
                assert_eq!(auth_return_target(Some(&target)), None, "{target:?}");
            }
        }
    }
}
